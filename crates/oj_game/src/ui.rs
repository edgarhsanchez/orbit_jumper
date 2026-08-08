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
    /// "4 TI · 2 ICE · 1 SP" — this row's own recipe, never a shared
    /// currency banner (a shared one made every row repaint at once).
    cost: String,
    /// Cost text color: lit when affordable, dim when not.
    cost_color: String,
    color: String,
    /// Slot index, passed back as the craft command's parameter.
    param: String,
}

/// One cockpit contact row: targeting data for a raider.
#[derive(Reflect, Clone, PartialEq, Default)]
struct ContactVm {
    name: String,
    /// "DST 1.42 GM · VEL 310 KM/S · CLS +12"
    data: String,
    color: String,
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
    plan: f64,
    sp_line: String,
    repair_line: String,
    repair_color: String,
    death_stats: String,
    style: String,
    threat: String,
    heading: String,
    contacts: Vec<ContactVm>,
    weapon_hints: String,
    target: String,
    arm: String,
    hull_warn: String,
}

#[derive(Resource, Clone)]
struct HudModel(Bindable);

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiLayoutState>()
            .add_systems(Startup, init_model)
            .add_systems(
                Update,
                (update_hud, toggle_panel, log_commands, exit_orbit_visibility, death_screen_visibility),
            )
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

/// True on devices that drive the game by touch (phones, tablets,
/// touch laptops). On the web this reads the browser's own capability
/// report; native builds are keyboard/mouse machines.
fn touch_device() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .map(|w| w.navigator().max_touch_points() > 0)
            .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    false
}

#[derive(Resource, Default)]
struct UiLayoutState(Option<(UiMode, crate::sim::ViewMode, Armament)>);

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
    mut sfx: MessageWriter<crate::audio::Sfx>,
) {
    for (interaction, key) in &controls {
        match interaction {
            Interaction::Pressed => {
                keys.press(key.0);
                sfx.write(crate::audio::Sfx::Click);
            }
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
      <TextBlock Text="{Binding hull_warn}" Foreground="#FF5459" FontSize="10" Margin="0,4,0,0">
        <TextBlock.Triggers>
          <EventTrigger RoutedEvent="Loaded">
            <BeginStoryboard>
              <Storyboard>
                <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                 From="1.0" To="0.25" Duration="0:0:0.35"
                                 RepeatBehavior="Forever" AutoReverse="True"/>
              </Storyboard>
            </BeginStoryboard>
          </EventTrigger>
        </TextBlock.Triggers>
      </TextBlock>
    </StackPanel>
  </Border>

  <Border Background="#F00D131C" BorderBrush="#1E3A44" BorderThickness="1" Padding="10,8" Width="236" Margin="0,8,0,0">
    <StackPanel>
      <TextBlock Text="{Binding nav}" Foreground="#00E5FF" FontSize="10"/>
      <ProgressBar Width="214" Height="3" Maximum="100" Value="{Binding plan}"
                   Foreground="#00E5FF" Background="#0A1420" BorderBrush="#16222E" Margin="0,3,0,0"/>
      <TextBlock Text="{Binding arm}" Foreground="#FFB454" FontSize="10"/>
      <TextBlock Text="{Binding threat}" Foreground="#FF5459" FontSize="10">
      <TextBlock.Triggers>
        <EventTrigger RoutedEvent="Loaded">
          <BeginStoryboard>
            <Storyboard>
              <DoubleAnimation Storyboard.TargetProperty="Opacity"
                               From="1.0" To="0.3" Duration="0:0:0.45"
                               RepeatBehavior="Forever" AutoReverse="True"/>
            </Storyboard>
          </BeginStoryboard>
        </EventTrigger>
      </TextBlock.Triggers>
      </TextBlock>
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
    <Rectangle Width="@PS" Height="1" Fill="#00E5FF" Margin="0,5,0,8" HorizontalAlignment="Left">
      <Rectangle.Triggers>
        <EventTrigger RoutedEvent="Loaded">
          <BeginStoryboard>
            <Storyboard>
              <DoubleAnimation Storyboard.TargetProperty="Width"
                               From="0" To="@PS" Duration="0:0:0.45"
                               FillBehavior="Stop"/>
            </Storyboard>
          </BeginStoryboard>
        </EventTrigger>
      </Rectangle.Triggers>
    </Rectangle>

    <TextBlock Text="{Binding sp_line}" Foreground="#00E5FF" FontSize="11" Margin="0,0,0,4"/>

    <StackPanel Orientation="Horizontal" Margin="0,0,0,6">
      <TextBlock Text="{Binding repair_line}" Foreground="{Binding repair_color}"
                 FontSize="11" Width="231" Margin="0,4,0,0"/>
      <Button Content="REPAIR" Command="repair"
              Background="#0B1B22" BorderBrush="#FF8A50" Foreground="#FF8A50"
              FontSize="10" Padding="7,2"/>
    </StackPanel>

    <ItemsControl ItemsSource="{Binding rows}">
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <StackPanel Orientation="Horizontal" Margin="0,4,0,0">
            <Rectangle Width="9" Height="9" Fill="{Binding color}" Margin="0,4,0,0"/>
            <StackPanel Width="128" Margin="9,0,0,0">
              <TextBlock Text="{Binding name}" Foreground="#E0E2EB" FontSize="11"/>
              <TextBlock Text="{Binding tier}" Foreground="#5A6472" FontSize="10"/>
            </StackPanel>
            <TextBlock Text="{Binding cost}" Foreground="{Binding cost_color}" FontSize="10" Width="94" Margin="0,4,0,0"/>
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
          <TextBlock Text="{Binding}" Foreground="#5A6472" FontSize="10" Margin="0,2,0,0">
            <TextBlock.Triggers>
              <EventTrigger RoutedEvent="Loaded">
                <BeginStoryboard>
                  <Storyboard>
                    <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                     From="0.0" To="1.0" Duration="0:0:0.6"
                                     FillBehavior="Stop"/>
                  </Storyboard>
                </BeginStoryboard>
              </EventTrigger>
            </TextBlock.Triggers>
          </TextBlock>
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
        HorizontalAlignment="@TA" VerticalAlignment="Top" Margin="@TPM"
        Background="#D80B111A" BorderBrush="#1E3A44" BorderThickness="1" Padding="14,4">
  <TextBlock Text="{Binding heading}" Foreground="#00E5FF" FontSize="12">
    <TextBlock.Triggers>
      <EventTrigger RoutedEvent="Loaded">
        <BeginStoryboard>
          <Storyboard>
            <ColorAnimation Storyboard.TargetProperty="Foreground"
                            From="#00E5FF" To="#B5F6FF" Duration="0:0:1.8"
                            RepeatBehavior="Forever" AutoReverse="True"/>
          </Storyboard>
        </BeginStoryboard>
      </EventTrigger>
    </TextBlock.Triggers>
  </TextBlock>
</Border>
"##;

const COCKPIT_RETICLE_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Center" VerticalAlignment="Center">
  <Ellipse Width="54" Height="54" Stroke="#8000E5FF" StrokeThickness="1" Fill="#00000000">
    <Ellipse.Triggers>
      <EventTrigger RoutedEvent="Loaded">
        <BeginStoryboard>
          <Storyboard>
            <DoubleAnimation Storyboard.TargetProperty="Width"
                             From="50" To="58" Duration="0:0:1.6"
                             RepeatBehavior="Forever" AutoReverse="True"/>
            <DoubleAnimation Storyboard.TargetProperty="Height"
                             From="50" To="58" Duration="0:0:1.6"
                             RepeatBehavior="Forever" AutoReverse="True"/>
            <DoubleAnimation Storyboard.TargetProperty="Opacity"
                             From="0.55" To="1.0" Duration="0:0:1.6"
                             RepeatBehavior="Forever" AutoReverse="True"/>
          </Storyboard>
        </BeginStoryboard>
      </EventTrigger>
    </Ellipse.Triggers>
  </Ellipse>
</StackPanel>
"##;

const COCKPIT_THREAT_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Right" VerticalAlignment="Top" Margin="@HM">
  <TextBlock Text="{Binding threat}" Foreground="#FF5459" FontSize="11">
  <TextBlock.Triggers>
        <EventTrigger RoutedEvent="Loaded">
          <BeginStoryboard>
            <Storyboard>
              <DoubleAnimation Storyboard.TargetProperty="Opacity"
                               From="1.0" To="0.3" Duration="0:0:0.45"
                               RepeatBehavior="Forever" AutoReverse="True"/>
            </Storyboard>
          </BeginStoryboard>
        </EventTrigger>
      </TextBlock.Triggers>
  </TextBlock>
</StackPanel>
"##;

const COCKPIT_CONTACTS_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="@CA" VerticalAlignment="Top" Margin="@CM"
        Background="#D80B111A" BorderBrush="#1E3A44" BorderThickness="1" Padding="10,6" Width="@CW">
  <StackPanel>
    <TextBlock Text="CONTACTS" Foreground="#5A6472" FontSize="10"/>
    <ItemsControl ItemsSource="{Binding contacts}">
      <ItemsControl.ItemTemplate>
        <DataTemplate>
          <StackPanel Margin="0,4,0,0">
            <TextBlock Text="{Binding name}" Foreground="{Binding color}" FontSize="11"/>
            <TextBlock Text="{Binding data}" Foreground="#8A93A0" FontSize="10"/>
          </StackPanel>
        </DataTemplate>
      </ItemsControl.ItemTemplate>
    </ItemsControl>
  </StackPanel>
</Border>
"##;

const COCKPIT_CONSOLE_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="Center" VerticalAlignment="Bottom" Margin="@KM"
        Background="#D80B111A" BorderBrush="#1E3A44" BorderThickness="1" Padding="18,6">
  <Border.Triggers>
    <EventTrigger RoutedEvent="Loaded">
      <BeginStoryboard>
        <Storyboard>
          <ColorAnimation Storyboard.TargetProperty="BorderBrush"
                          From="#1E3A44" To="#00E5FF" Duration="0:0:2.6"
                          RepeatBehavior="Forever" AutoReverse="True"/>
        </Storyboard>
      </BeginStoryboard>
    </EventTrigger>
  </Border.Triggers>
  <StackPanel>
    <TextBlock Text="{Binding speed}" Foreground="#E0E2EB" FontSize="16" FontWeight="Bold"
               HorizontalAlignment="Center"/>
    <TextBlock Text="{Binding nav}" Foreground="#00E5FF" FontSize="10" Margin="0,2,0,0"/>
    <ProgressBar Width="214" Height="3" Maximum="100" Value="{Binding plan}"
                 Foreground="#00E5FF" Background="#0A1420" BorderBrush="#16222E" Margin="0,3,0,0"/>
    <TextBlock Text="{Binding target}" Foreground="#FF5459" FontSize="10" Margin="0,2,0,0"
               HorizontalAlignment="Center">
    <TextBlock.Triggers>
        <EventTrigger RoutedEvent="Loaded">
          <BeginStoryboard>
            <Storyboard>
              <DoubleAnimation Storyboard.TargetProperty="Opacity"
                               From="1.0" To="0.45" Duration="0:0:0.3"
                               RepeatBehavior="Forever" AutoReverse="True"/>
            </Storyboard>
          </BeginStoryboard>
        </EventTrigger>
      </TextBlock.Triggers>
    </TextBlock>
    <TextBlock Text="{Binding arm}" Foreground="#FFB454" FontSize="10" Margin="0,2,0,0"
               HorizontalAlignment="Center"/>
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
      <TextBlock Text="{Binding hull_warn}" Foreground="#FF5459" FontSize="10" Margin="0,4,0,0">
        <TextBlock.Triggers>
          <EventTrigger RoutedEvent="Loaded">
            <BeginStoryboard>
              <Storyboard>
                <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                 From="1.0" To="0.25" Duration="0:0:0.35"
                                 RepeatBehavior="Forever" AutoReverse="True"/>
              </Storyboard>
            </BeginStoryboard>
          </EventTrigger>
        </TextBlock.Triggers>
      </TextBlock>
    </StackPanel>
  </Border>
  <TextBlock Text="{Binding weapon_hints}" Foreground="#3A4650" FontSize="10" Margin="2,8,0,0"/>
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
  <StackPanel.Resources>
    <Style x:Key="con-cyan" TargetType="Button">
      <Setter Property="Template">
        <Setter.Value>
          <ControlTemplate TargetType="Button">
            <Grid Width="66" Height="42">
              <Path x:Name="frame" Width="66" Height="42" Stretch="Fill"
                    Fill="#D8071018" Stroke="#00E5FF" StrokeThickness="1"
                    Data="M 10,0 L 56,0 L 66,10 L 66,42 L 56,52 L 10,52 L 0,42 L 0,10 Z"/>
              <Rectangle x:Name="notch" Width="26" Height="2" Fill="#00E5FF"
                         HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12,0,0,0"/>
              <Ellipse x:Name="led" Width="5" Height="5" Fill="#00E5FF"
                       HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,6,9,0">
                <Ellipse.Triggers>
                  <EventTrigger RoutedEvent="Loaded">
                    <BeginStoryboard>
                      <Storyboard RepeatBehavior="Forever" AutoReverse="True">
                        <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                         From="0.2" To="1.0" Duration="0:0:1.1"/>
                      </Storyboard>
                    </BeginStoryboard>
                  </EventTrigger>
                </Ellipse.Triggers>
              </Ellipse>
              <ContentPresenter/>
            </Grid>
            <ControlTemplate.Triggers>
              <Trigger Property="IsMouseOver" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#5900E5FF"/>
                <Setter TargetName="notch" Property="Width" Value="44"/>
              </Trigger>
              <Trigger Property="IsPressed" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#C800E5FF"/>
                <Setter TargetName="notch" Property="Width" Value="56"/>
              </Trigger>
            </ControlTemplate.Triggers>
          </ControlTemplate>
        </Setter.Value>
      </Setter>
    </Style>
    <Style x:Key="con-amber" TargetType="Button">
      <Setter Property="Template">
        <Setter.Value>
          <ControlTemplate TargetType="Button">
            <Grid Width="66" Height="42">
              <Path x:Name="frame" Width="66" Height="42" Stretch="Fill"
                    Fill="#D8141008" Stroke="#FFB454" StrokeThickness="1"
                    Data="M 10,0 L 56,0 L 66,10 L 66,42 L 56,52 L 10,52 L 0,42 L 0,10 Z"/>
              <Rectangle x:Name="notch" Width="26" Height="2" Fill="#FFB454"
                         HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12,0,0,0"/>
              <Ellipse x:Name="led" Width="5" Height="5" Fill="#FFB454"
                       HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,6,9,0">
                <Ellipse.Triggers>
                  <EventTrigger RoutedEvent="Loaded">
                    <BeginStoryboard>
                      <Storyboard RepeatBehavior="Forever" AutoReverse="True">
                        <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                         From="0.2" To="1.0" Duration="0:0:1.1"/>
                      </Storyboard>
                    </BeginStoryboard>
                  </EventTrigger>
                </Ellipse.Triggers>
              </Ellipse>
              <ContentPresenter/>
            </Grid>
            <ControlTemplate.Triggers>
              <Trigger Property="IsMouseOver" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#59FFB454"/>
                <Setter TargetName="notch" Property="Width" Value="44"/>
              </Trigger>
              <Trigger Property="IsPressed" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#C8FFB454"/>
                <Setter TargetName="notch" Property="Width" Value="56"/>
              </Trigger>
            </ControlTemplate.Triggers>
          </ControlTemplate>
        </Setter.Value>
      </Setter>
    </Style>
    <Style x:Key="con-violet-t" TargetType="Button">
      <Setter Property="Template">
        <Setter.Value>
          <ControlTemplate TargetType="Button">
            <Grid Width="66" Height="42">
              <Path x:Name="frame" Width="66" Height="42" Stretch="Fill"
                    Fill="#D80E0A18" Stroke="#B48CFF" StrokeThickness="1"
                    Data="M 10,0 L 56,0 L 66,10 L 66,42 L 56,52 L 10,52 L 0,42 L 0,10 Z"/>
              <Rectangle x:Name="notch" Width="26" Height="2" Fill="#B48CFF"
                         HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12,0,0,0"/>
              <Ellipse x:Name="led" Width="5" Height="5" Fill="#B48CFF"
                       HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,6,9,0">
                <Ellipse.Triggers>
                  <EventTrigger RoutedEvent="Loaded">
                    <BeginStoryboard>
                      <Storyboard RepeatBehavior="Forever" AutoReverse="True">
                        <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                         From="0.2" To="1.0" Duration="0:0:1.1"/>
                      </Storyboard>
                    </BeginStoryboard>
                  </EventTrigger>
                </Ellipse.Triggers>
              </Ellipse>
              <ContentPresenter/>
            </Grid>
            <ControlTemplate.Triggers>
              <Trigger Property="IsMouseOver" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#59B48CFF"/>
                <Setter TargetName="notch" Property="Width" Value="44"/>
              </Trigger>
              <Trigger Property="IsPressed" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#C8B48CFF"/>
                <Setter TargetName="notch" Property="Width" Value="56"/>
              </Trigger>
            </ControlTemplate.Triggers>
          </ControlTemplate>
        </Setter.Value>
      </Setter>
    </Style>
  </StackPanel.Resources>
    <Button x:Name="btn_vessel" Style="{StaticResource con-cyan}" Margin="0,0,6,0">
      <StackPanel>
        <TextBlock Text="VESSEL" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="TAB" Foreground="#3E5A66" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
    <Button x:Name="btn_map" Style="{StaticResource con-cyan}" Margin="0,0,6,0">
      <StackPanel>
        <TextBlock Text="MAP" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="M" Foreground="#3E5A66" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
    <Button x:Name="btn_study" Style="{StaticResource con-amber}" Margin="0,0,6,0">
      <StackPanel>
        <TextBlock Text="STUDY" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="HOLD S" Foreground="#6A5A3E" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
    <Button x:Name="btn_view" Style="{StaticResource con-violet-t}" Margin="0,0,0,0">
      <StackPanel>
        <TextBlock Text="VIEW" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="F" Foreground="#55496A" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
</StackPanel>
"##;

const TOUCH_TOPBAR_COCKPIT_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,8,8,0"
            Orientation="Horizontal">
  <StackPanel.Resources>
    <Style x:Key="con-violet-t" TargetType="Button">
      <Setter Property="Template">
        <Setter.Value>
          <ControlTemplate TargetType="Button">
            <Grid Width="66" Height="42">
              <Path x:Name="frame" Width="66" Height="42" Stretch="Fill"
                    Fill="#D80E0A18" Stroke="#B48CFF" StrokeThickness="1"
                    Data="M 10,0 L 56,0 L 66,10 L 66,42 L 56,52 L 10,52 L 0,42 L 0,10 Z"/>
              <Rectangle x:Name="notch" Width="26" Height="2" Fill="#B48CFF"
                         HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12,0,0,0"/>
              <Ellipse x:Name="led" Width="5" Height="5" Fill="#B48CFF"
                       HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,6,9,0">
                <Ellipse.Triggers>
                  <EventTrigger RoutedEvent="Loaded">
                    <BeginStoryboard>
                      <Storyboard RepeatBehavior="Forever" AutoReverse="True">
                        <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                         From="0.2" To="1.0" Duration="0:0:1.1"/>
                      </Storyboard>
                    </BeginStoryboard>
                  </EventTrigger>
                </Ellipse.Triggers>
              </Ellipse>
              <ContentPresenter/>
            </Grid>
            <ControlTemplate.Triggers>
              <Trigger Property="IsMouseOver" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#59B48CFF"/>
                <Setter TargetName="notch" Property="Width" Value="44"/>
              </Trigger>
              <Trigger Property="IsPressed" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#C8B48CFF"/>
                <Setter TargetName="notch" Property="Width" Value="56"/>
              </Trigger>
            </ControlTemplate.Triggers>
          </ControlTemplate>
        </Setter.Value>
      </Setter>
    </Style>
  </StackPanel.Resources>
    <Button x:Name="btn_view" Style="{StaticResource con-violet-t}" Margin="0,0,0,0">
      <StackPanel>
        <TextBlock Text="VIEW" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="F" Foreground="#55496A" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
</StackPanel>
"##;

const TOUCH_THRUST_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Left" VerticalAlignment="Bottom" Margin="@TM">
  <StackPanel.Resources>
    <Style x:Key="con-teal" TargetType="Button">
      <Setter Property="Template">
        <Setter.Value>
          <ControlTemplate TargetType="Button">
            <Grid Width="66" Height="52">
              <Path x:Name="frame" Width="66" Height="52" Stretch="Fill"
                    Fill="#D8081514" Stroke="#00FFD4" StrokeThickness="1"
                    Data="M 10,0 L 56,0 L 66,10 L 66,42 L 56,52 L 10,52 L 0,42 L 0,10 Z"/>
              <Rectangle x:Name="notch" Width="26" Height="2" Fill="#00FFD4"
                         HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12,0,0,0"/>
              <Ellipse x:Name="led" Width="5" Height="5" Fill="#00FFD4"
                       HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,6,9,0">
                <Ellipse.Triggers>
                  <EventTrigger RoutedEvent="Loaded">
                    <BeginStoryboard>
                      <Storyboard RepeatBehavior="Forever" AutoReverse="True">
                        <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                         From="0.2" To="1.0" Duration="0:0:1.1"/>
                      </Storyboard>
                    </BeginStoryboard>
                  </EventTrigger>
                </Ellipse.Triggers>
              </Ellipse>
              <ContentPresenter/>
            </Grid>
            <ControlTemplate.Triggers>
              <Trigger Property="IsMouseOver" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#5900FFD4"/>
                <Setter TargetName="notch" Property="Width" Value="44"/>
              </Trigger>
              <Trigger Property="IsPressed" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#C800FFD4"/>
                <Setter TargetName="notch" Property="Width" Value="56"/>
              </Trigger>
            </ControlTemplate.Triggers>
          </ControlTemplate>
        </Setter.Value>
      </Setter>
    </Style>
  </StackPanel.Resources>
  <Border Background="#C8060C12" BorderBrush="#12333A" BorderThickness="1" Padding="8,6">
    <StackPanel>
      <StackPanel Orientation="Horizontal">
        <Rectangle Width="12" Height="2" Fill="#00FFD4" Margin="0,5,6,0">
          <Rectangle.Triggers>
            <EventTrigger RoutedEvent="Loaded">
              <BeginStoryboard>
                <Storyboard>
                  <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                   From="0.3" To="1.0" Duration="0:0:2.2"
                                   RepeatBehavior="Forever" AutoReverse="True"/>
                </Storyboard>
              </BeginStoryboard>
            </EventTrigger>
          </Rectangle.Triggers>
        </Rectangle>
        <TextBlock Text="NAV_CTRL_L" Foreground="#3E6A66" FontSize="9"/>
      </StackPanel>
      <StackPanel Orientation="Horizontal" Margin="0,6,0,0">
    <Button x:Name="btn_climb" Style="{StaticResource con-teal}" Margin="0,0,6,0">
      <StackPanel>
        <TextBlock Text="VERT+" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="ASC E" Foreground="#3E6A66" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
    <Button x:Name="btn_dive" Style="{StaticResource con-teal}" Margin="0,0,0,0">
      <StackPanel>
        <TextBlock Text="VERT-" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="DSC Q" Foreground="#3E6A66" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
      </StackPanel>
      <StackPanel Orientation="Horizontal" Margin="0,6,0,0">
    <Button x:Name="btn_arm" Style="{StaticResource con-teal}" Margin="0,0,0,0">
      <StackPanel>
        <TextBlock Text="SOLAR" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="ARM P" Foreground="#3E6A66" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
      </StackPanel>
    </StackPanel>
  </Border>
</StackPanel>
"##;

const TOUCH_WEAPONS_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Right" VerticalAlignment="Bottom" Margin="0,0,8,8">
  <StackPanel.Resources>
    <Style x:Key="con-orange" TargetType="Button">
      <Setter Property="Template">
        <Setter.Value>
          <ControlTemplate TargetType="Button">
            <Grid Width="66" Height="52">
              <Path x:Name="frame" Width="66" Height="52" Stretch="Fill"
                    Fill="#D8140B08" Stroke="#FF7043" StrokeThickness="1"
                    Data="M 10,0 L 56,0 L 66,10 L 66,42 L 56,52 L 10,52 L 0,42 L 0,10 Z"/>
              <Rectangle x:Name="notch" Width="26" Height="2" Fill="#FF7043"
                         HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12,0,0,0"/>
              <Ellipse x:Name="led" Width="5" Height="5" Fill="#FF7043"
                       HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,6,9,0">
                <Ellipse.Triggers>
                  <EventTrigger RoutedEvent="Loaded">
                    <BeginStoryboard>
                      <Storyboard RepeatBehavior="Forever" AutoReverse="True">
                        <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                         From="0.2" To="1.0" Duration="0:0:1.1"/>
                      </Storyboard>
                    </BeginStoryboard>
                  </EventTrigger>
                </Ellipse.Triggers>
              </Ellipse>
              <ContentPresenter/>
            </Grid>
            <ControlTemplate.Triggers>
              <Trigger Property="IsMouseOver" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#59FF7043"/>
                <Setter TargetName="notch" Property="Width" Value="44"/>
              </Trigger>
              <Trigger Property="IsPressed" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#C8FF7043"/>
                <Setter TargetName="notch" Property="Width" Value="56"/>
              </Trigger>
            </ControlTemplate.Triggers>
          </ControlTemplate>
        </Setter.Value>
      </Setter>
    </Style>
    <Style x:Key="con-violet" TargetType="Button">
      <Setter Property="Template">
        <Setter.Value>
          <ControlTemplate TargetType="Button">
            <Grid Width="66" Height="52">
              <Path x:Name="frame" Width="66" Height="52" Stretch="Fill"
                    Fill="#D80E0A18" Stroke="#B48CFF" StrokeThickness="1"
                    Data="M 10,0 L 56,0 L 66,10 L 66,42 L 56,52 L 10,52 L 0,42 L 0,10 Z"/>
              <Rectangle x:Name="notch" Width="26" Height="2" Fill="#B48CFF"
                         HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12,0,0,0"/>
              <Ellipse x:Name="led" Width="5" Height="5" Fill="#B48CFF"
                       HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,6,9,0">
                <Ellipse.Triggers>
                  <EventTrigger RoutedEvent="Loaded">
                    <BeginStoryboard>
                      <Storyboard RepeatBehavior="Forever" AutoReverse="True">
                        <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                         From="0.2" To="1.0" Duration="0:0:1.1"/>
                      </Storyboard>
                    </BeginStoryboard>
                  </EventTrigger>
                </Ellipse.Triggers>
              </Ellipse>
              <ContentPresenter/>
            </Grid>
            <ControlTemplate.Triggers>
              <Trigger Property="IsMouseOver" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#59B48CFF"/>
                <Setter TargetName="notch" Property="Width" Value="44"/>
              </Trigger>
              <Trigger Property="IsPressed" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#C8B48CFF"/>
                <Setter TargetName="notch" Property="Width" Value="56"/>
              </Trigger>
            </ControlTemplate.Triggers>
          </ControlTemplate>
        </Setter.Value>
      </Setter>
    </Style>
    <Style x:Key="con-shield" TargetType="Button">
      <Setter Property="Template">
        <Setter.Value>
          <ControlTemplate TargetType="Button">
            <Grid Width="66" Height="52">
              <Path x:Name="frame" Width="66" Height="52" Stretch="Fill"
                    Fill="#D8061218" Stroke="#00E5FF" StrokeThickness="1"
                    Data="M 10,0 L 56,0 L 66,10 L 66,42 L 56,52 L 10,52 L 0,42 L 0,10 Z"/>
              <Rectangle x:Name="notch" Width="26" Height="2" Fill="#00E5FF"
                         HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12,0,0,0"/>
              <Ellipse x:Name="led" Width="5" Height="5" Fill="#00E5FF"
                       HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,6,9,0">
                <Ellipse.Triggers>
                  <EventTrigger RoutedEvent="Loaded">
                    <BeginStoryboard>
                      <Storyboard RepeatBehavior="Forever" AutoReverse="True">
                        <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                         From="0.2" To="1.0" Duration="0:0:1.1"/>
                      </Storyboard>
                    </BeginStoryboard>
                  </EventTrigger>
                </Ellipse.Triggers>
              </Ellipse>
              <ContentPresenter/>
            </Grid>
            <ControlTemplate.Triggers>
              <Trigger Property="IsMouseOver" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#5900E5FF"/>
                <Setter TargetName="notch" Property="Width" Value="44"/>
              </Trigger>
              <Trigger Property="IsPressed" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#C800E5FF"/>
                <Setter TargetName="notch" Property="Width" Value="56"/>
              </Trigger>
            </ControlTemplate.Triggers>
          </ControlTemplate>
        </Setter.Value>
      </Setter>
    </Style>
  </StackPanel.Resources>
  <Border Background="#C8060C12" BorderBrush="#12333A" BorderThickness="1" Padding="8,6">
    <StackPanel>
      <StackPanel Orientation="Horizontal">
        <Rectangle Width="12" Height="2" Fill="#FF7043" Margin="0,5,6,0">
          <Rectangle.Triggers>
            <EventTrigger RoutedEvent="Loaded">
              <BeginStoryboard>
                <Storyboard>
                  <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                   From="0.3" To="1.0" Duration="0:0:2.2"
                                   RepeatBehavior="Forever" AutoReverse="True"/>
                </Storyboard>
              </BeginStoryboard>
            </EventTrigger>
          </Rectangle.Triggers>
        </Rectangle>
        <TextBlock Text="WPN_SYST_R" Foreground="#6A4A3E" FontSize="9"/>
      </StackPanel>
      @WROW1
      @WROW2
      @WROW3
    </StackPanel>
  </Border>
</StackPanel>
"##;

// Weapon buttons exist only once their system is CRAFTED. Each row below
// is substituted into TOUCH_WEAPONS_XAML per the installed tiers; an
// unarmed vessel gets no weapons cluster at all.
const WPN_LAS_BTN: &str = r##"
    <Button x:Name="btn_las" Style="{StaticResource con-orange}" Margin="0,0,6,0">
      <StackPanel>
        <TextBlock Text="LAS" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="HOLD Z" Foreground="#6A4A3E" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
"##;

const WPN_MSL_BTN: &str = r##"
    <Button x:Name="btn_msl" Style="{StaticResource con-orange}" Margin="0,0,0,0">
      <StackPanel>
        <TextBlock Text="MSL" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="X" Foreground="#6A4A3E" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
"##;

const WPN_NOVA_ROW: &str = r##"
    <Button x:Name="btn_nova" Style="{StaticResource con-shield}" Margin="0,0,0,0">
      <StackPanel>
        <TextBlock Text="NOVA" Foreground="#E8F8FF" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="SHIELD N" Foreground="#2A5A66" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
"##;

const WPN_WELL_ROW: &str = r##"
    <Button x:Name="btn_pull" Style="{StaticResource con-violet}" Margin="0,0,6,0">
      <StackPanel>
        <TextBlock Text="PULL" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="WELL C" Foreground="#55496A" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
    <Button x:Name="btn_push" Style="{StaticResource con-violet}" Margin="0,0,0,0">
      <StackPanel>
        <TextBlock Text="PUSH" Foreground="#E8F4F8" FontSize="12" HorizontalAlignment="Center"/>
        <TextBlock Text="WELL V" Foreground="#55496A" FontSize="8" HorizontalAlignment="Center"/>
      </StackPanel>
    </Button>
"##;

// Shown only while riding an orbit: the sticky orbit's one-tap release.
const EXIT_ORBIT_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            HorizontalAlignment="Center" VerticalAlignment="Bottom" Margin="@XM">
  <StackPanel.Resources>
    <Style x:Key="con-exit" TargetType="Button">
      <Setter Property="Template">
        <Setter.Value>
          <ControlTemplate TargetType="Button">
            <Grid Width="96" Height="44">
              <Grid.Triggers>
                <EventTrigger RoutedEvent="Loaded">
                  <BeginStoryboard>
                    <Storyboard>
                      <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                       From="1.0" To="0.68" Duration="0:0:0.45"
                                       RepeatBehavior="Forever" AutoReverse="True"/>
                    </Storyboard>
                  </BeginStoryboard>
                </EventTrigger>
              </Grid.Triggers>
              <Path x:Name="frame" Width="96" Height="44" Stretch="Fill"
                    Fill="#D8120A08" Stroke="#FFB454" StrokeThickness="1"
                    Data="M 10,0 L 86,0 L 96,10 L 96,34 L 86,44 L 10,44 L 0,34 L 0,10 Z"/>
              <Rectangle x:Name="notch" Width="30" Height="2" Fill="#FFB454"
                         HorizontalAlignment="Left" VerticalAlignment="Top" Margin="12,0,0,0"/>
              <Ellipse x:Name="led" Width="5" Height="5" Fill="#FFB454"
                       HorizontalAlignment="Right" VerticalAlignment="Top" Margin="0,6,9,0">
                <Ellipse.Triggers>
                  <EventTrigger RoutedEvent="Loaded">
                    <BeginStoryboard>
                      <Storyboard RepeatBehavior="Forever" AutoReverse="True">
                        <DoubleAnimation Storyboard.TargetProperty="Opacity"
                                         From="0.2" To="1.0" Duration="0:0:1.1"/>
                      </Storyboard>
                    </BeginStoryboard>
                  </EventTrigger>
                </Ellipse.Triggers>
              </Ellipse>
              <ContentPresenter/>
            </Grid>
            <ControlTemplate.Triggers>
              <Trigger Property="IsMouseOver" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#59FFB454"/>
                <Setter TargetName="notch" Property="Width" Value="44"/>
              </Trigger>
              <Trigger Property="IsPressed" Value="True">
                <Setter TargetName="frame" Property="Fill" Value="#C8FFB454"/>
                <Setter TargetName="notch" Property="Width" Value="56"/>
              </Trigger>
            </ControlTemplate.Triggers>
          </ControlTemplate>
        </Setter.Value>
      </Setter>
    </Style>
  </StackPanel.Resources>
  <Button x:Name="btn_exit_orbit" Style="{StaticResource con-exit}">
    <StackPanel>
      <TextBlock Text="EXIT ORBIT" Foreground="#FFE8C8" FontSize="12" HorizontalAlignment="Center"/>
      <TextBlock Text="O" Foreground="#6A5A3E" FontSize="8" HorizontalAlignment="Center"/>
    </StackPanel>
  </Button>
</StackPanel>
"##;

/// Armament fingerprint: which weapon systems are installed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Armament {
    laser: bool,
    missiles: bool,
    wells: bool,
    nova: bool,
}

impl Armament {
    fn of(upgrades: &crate::upgrades::ShipUpgrades) -> Self {
        Self {
            laser: upgrades.tier(UpgradeSlot::LaserWeapon) > 0,
            missiles: upgrades.tier(UpgradeSlot::MissileRack) > 0,
            wells: upgrades.tier(UpgradeSlot::ForceFieldProjector) > 0,
            nova: upgrades.tier(UpgradeSlot::Shield) > 0,
        }
    }

    fn any(&self) -> bool {
        self.laser || self.missiles || self.wells || self.nova
    }

    /// The weapons cluster for this armament, or None when unarmed.
    fn weapons_xaml(&self, m: &Metrics) -> Option<String> {
        if !self.any() {
            return None;
        }
        let mut row1 = String::new();
        if self.laser {
            row1.push_str(WPN_LAS_BTN);
        }
        if self.missiles {
            row1.push_str(WPN_MSL_BTN);
        }
        let wrap = |inner: &str| {
            if inner.is_empty() {
                String::new()
            } else {
                format!("<StackPanel Orientation=\"Horizontal\" Margin=\"0,6,0,0\">{inner}</StackPanel>")
            }
        };
        let row2 = if self.wells { WPN_WELL_ROW } else { "" };
        let row3 = if self.nova { WPN_NOVA_ROW } else { "" };
        Some(
            m.fill(TOUCH_WEAPONS_XAML)
                .replace("@WROW1", &wrap(&row1))
                .replace("@WROW2", &wrap(row2))
                .replace("@WROW3", &wrap(row3)),
        )
    }

    /// The cockpit hint line only advertises what is actually installed.
    fn hints(&self) -> String {
        let mut s = String::from("[F] TACTICAL  [E/Q] VERT  [P] SOLAR");
        if self.laser {
            s.push_str("  [Z] LASER");
        }
        if self.missiles {
            s.push_str("  [X] MSL");
        }
        if self.wells {
            s.push_str("  [C/V] WELLS");
        }
        if self.nova {
            s.push_str("  [N] NOVA");
        }
        s
    }
}

/// btn_* name -> virtual key, for wiring TouchKey after instantiation.
const TOUCH_KEYS: [(&str, KeyCode); 13] = [
    ("btn_exit_orbit", KeyCode::KeyO),
    ("btn_arm", KeyCode::KeyP),
    ("btn_view", KeyCode::KeyF),
    ("btn_climb", KeyCode::KeyE),
    ("btn_dive", KeyCode::KeyQ),
    ("btn_vessel", KeyCode::Tab),
    ("btn_map", KeyCode::KeyM),
    ("btn_study", KeyCode::KeyS),
    ("btn_las", KeyCode::KeyZ),
    ("btn_msl", KeyCode::KeyX),
    ("btn_pull", KeyCode::KeyC),
    ("btn_push", KeyCode::KeyV),
    ("btn_nova", KeyCode::KeyN),
];

/// Root entity of the vessel panel, for the Tab toggle.
#[derive(Resource)]
struct VesselPanel(Entity);

/// Root entity of the galaxy map, for the M toggle.
#[derive(Resource)]
struct MapPanel(Entity);

/// Root entity of the EXIT ORBIT button; shown only while riding.
#[derive(Resource)]
struct ExitOrbitPanel(Entity);

/// Root entity of the destroyed-vessel screen; shown while a restart is
/// awaited.
#[derive(Resource)]
struct DeathPanel(Entity);

/// The camera control cluster, named for the drag systems: `pan_knob`
/// inside the yellow pan pad, `zoom_knob` riding the darker-blue
/// vertical zoom track.
#[derive(Resource)]
pub struct PanZoomUi {
    pub pan_knob: Entity,
    pub zoom_knob: Entity,
}

/// Camera controls, Stitch terminal styling: a pan pad shaped like the
/// NAV stick but smaller and in the game's yellow (#FFB454), with a
/// thin zoom slider to its LEFT in a darker blue. Geometry constants
/// live in stick.rs (`PAN_*`/`ZOOMBAR_*`) — the markup and the drag
/// math must agree.
const PAN_ZOOM_XAML: &str = r##"
<StackPanel xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
            Orientation="Horizontal" HorizontalAlignment="Right" VerticalAlignment="Bottom"
            Margin="0,0,110,12">
  <Border Width="14" Height="96" Background="#D00A1420" BorderBrush="#1B6FA8"
          BorderThickness="1" CornerRadius="7" Margin="0,0,8,0" VerticalAlignment="Bottom">
    <Border x:Name="zoom_knob" Width="8" Height="12" Background="#1B6FA8" CornerRadius="4"
            HorizontalAlignment="Center" VerticalAlignment="Top"/>
  </Border>
  <Border Width="84" Height="84" Background="#D00D131C" BorderBrush="#FFB454"
          BorderThickness="1" CornerRadius="42" VerticalAlignment="Bottom">
    <Border x:Name="pan_knob" Width="30" Height="30" Background="#40FFB454" BorderBrush="#FFB454"
            BorderThickness="1" CornerRadius="15"
            HorizontalAlignment="Center" VerticalAlignment="Center"/>
  </Border>
</StackPanel>
"##;

/// The destroyed-vessel screen: what the run was worth, what survives,
/// and the invitation to fly again. Enter works too.
const DEATH_XAML: &str = r##"
<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        HorizontalAlignment="Center" VerticalAlignment="Center"
        Background="#F60D131C" BorderBrush="#FF5459" BorderThickness="1"
        Padding="26,20" Width="430">
  <StackPanel>
    <TextBlock Text="VESSEL DESTROYED" Foreground="#FF5459" FontSize="20"/>
    <Rectangle Width="378" Height="1" Fill="#FF5459" Margin="0,8,0,10" HorizontalAlignment="Left"/>
    <TextBlock Text="{Binding death_stats}" Foreground="#E0E2EB" FontSize="12"/>
    <TextBlock Text="HULL AND RANK ARE LOST — A NEW RUN STARTS AT LEVEL 1. GEAR, STASH AND CAREER RECORDS SURVIVE."
               Foreground="#5A6472" FontSize="10" Margin="0,8,0,0"/>
    <Button Content="START OVER" Command="restart" Margin="0,14,0,0"
            Background="#1A0E12" BorderBrush="#FF5459" Foreground="#FF5459"
            FontSize="13" Padding="16,7" HorizontalAlignment="Left"/>
  </StackPanel>
</Border>
"##;

/// Show the destroyed-vessel screen while a restart is awaited; keep its
/// frozen run summary bound.
fn death_screen_visibility(
    panel: Option<Res<DeathPanel>>,
    last_run: Res<crate::modules::LastRun>,
    model: Option<Res<HudModel>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut awaiting: ResMut<crate::modules::AwaitingRestart>,
    mut vis: Query<&mut Visibility>,
) {
    if let Some(model) = model {
        model.0.set_death_stats(last_run.0.clone());
    }
    // Enter restarts without reaching for the mouse.
    if awaiting.0 && keys.just_pressed(KeyCode::Enter) {
        awaiting.0 = false;
    }
    let Some(panel) = panel else { return };
    if let Ok(mut v) = vis.get_mut(panel.0) {
        *v = if awaiting.0 { Visibility::Visible } else { Visibility::Hidden };
    }
}

fn exit_orbit_visibility(
    panel: Option<Res<ExitOrbitPanel>>,
    ships: Query<&NavState, With<crate::sim::Ship>>,
    mut vis: Query<&mut Visibility>,
) {
    let (Some(panel), Ok(nav)) = (panel, ships.single()) else { return };
    if let Ok(mut v) = vis.get_mut(panel.0) {
        let want = if matches!(nav, NavState::Orbiting { .. }) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
    }
}

/// Craftable slots, in vessel-panel row order; the craft command's
/// parameter indexes this.
const CRAFT_SLOTS: [(UpgradeSlot, &str, &str); 10] = [
    (UpgradeSlot::Shield, "SHIELD PLATING", "#00A2FF"),
    (UpgradeSlot::CommandArray, "COMMAND ARRAY", "#B48CFF"),
    (UpgradeSlot::RocketDrive, "ROCKET DRIVE", "#FF7043"),
    (UpgradeSlot::EnergyCollector, "COLLECTOR", "#00FFD4"),
    (UpgradeSlot::GravityDrive, "GRAVITY DRIVE", "#FF4FD8"),
    (UpgradeSlot::LaserWeapon, "LASER ARRAY", "#FF5459"),
    (UpgradeSlot::MissileRack, "MISSILE RACK", "#FFB454"),
    (UpgradeSlot::ForceFieldProjector, "WELL PROJECTOR", "#B48CFF"),
    (UpgradeSlot::Hull, "HULL PLATING", "#FF7043"),
    (UpgradeSlot::LightDrive, "LIGHT DRIVE", "#7E97B8"),
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
    // The panel's REPAIR button patches the hull from the stash.
    vm.on_command("repair", |world, _| {
        crate::upgrades::repair_from_world(world);
    });
    // The destroyed-vessel screen's START OVER: release the respawn.
    vm.on_command("restart", |world, _| {
        world.resource_mut::<crate::modules::AwaitingRestart>().0 = false;
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
    /// Cockpit contacts panel: alignment / margin / width. Portrait has
    /// no room beside the bars, so it stacks below them.
    contacts_align: String,
    contacts_margin: String,
    contacts_w: i32,
    /// Cockpit top row: heading tape alignment/margin, threat margin.
    tape_align: String,
    tape_margin: String,
    threat_margin: String,
    console_margin: String,
    exit_margin: String,
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
                // Right of the virtual stick (24px margin + 132px pad).
                thrust_margin: "172,0,0,8".into(),
                contacts_align: "Right".into(),
                contacts_margin: "0,34,12,0".into(),
                contacts_w: 252,
                tape_align: "Center".into(),
                tape_margin: "0,10,0,0".into(),
                threat_margin: "0,10,12,0".into(),
                console_margin: "0,0,0,8".into(),
                exit_margin: "0,0,0,84".into(),
            },
            // Landscape phone: desktop arrangement (it fits), plus touch
            // controls in the corners.
            UiMode::PhoneLandscape => Self {
                touch_controls: true,
                // The stick sits right of the status column (x 260-392);
                // the vert cluster rides beside it.
                thrust_margin: "408,0,0,8".into(),
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
                    // Portrait: above the stick — beside it would collide
                    // with the right-anchored weapons cluster at 390px.
                    thrust_margin: "24,0,0,196".into(),
                    contacts_align: "Left".into(),
                    contacts_margin: "12,180,0,0".into(),
                    contacts_w: (win_w as i32 - 24).clamp(240, 366),
                    tape_align: "Right".into(),
                    tape_margin: "0,110,8,0".into(),
                    threat_margin: "0,80,8,0".into(),
                    console_margin: "0,0,0,178".into(),
                    exit_margin: "0,0,0,252".into(),
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
            .replace("@CA", &self.contacts_align)
            .replace("@CM", &self.contacts_margin)
            .replace("@CW", &self.contacts_w.to_string())
            .replace("@TA", &self.tape_align)
            .replace("@TPM", &self.tape_margin)
            .replace("@HM", &self.threat_margin)
            .replace("@KM", &self.console_margin)
            .replace("@XM", &self.exit_margin)
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
    // Crafting a weapon system rebuilds the control surface: buttons for
    // uninstalled weapons must not exist, not merely do nothing.
    let arm = Armament::of(world.resource::<crate::upgrades::ShipUpgrades>());
    if world.resource::<UiLayoutState>().0 == Some((mode, view, arm)) {
        return;
    }
    world.resource_mut::<UiLayoutState>().0 = Some((mode, view, arm));
    info!("ui layout: {mode:?} / {view:?} / {arm:?} ({w:.0}x{h:.0})");

    // Tear down the previous layout's documents.
    let old: Vec<Entity> = world
        .query_filtered::<Entity, With<UiRoot>>()
        .iter(world)
        .collect();
    for e in old {
        world.entity_mut(e).despawn();
    }

    let mut m = Metrics::for_mode(mode, w);
    // Resolution alone lies about input: an iPad (or touch laptop) at
    // desktop resolution still drives the game by touch and needs the
    // on-screen controls a phone gets. Capability, not size.
    if touch_device() {
        m.touch_controls = true;
    }
    enum Doc {
        Hud,
        Vessel,
        Map,
        Controls,
        ExitOrbit,
        Death,
        PanZoom,
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
        let mut docs = vec![
            (m.fill(HUD_COCKPIT_XAML), Doc::Hud),
            (m.fill(COCKPIT_TAPE_XAML), Doc::Hud),
            (m.fill(COCKPIT_RETICLE_XAML), Doc::Hud),
            (m.fill(COCKPIT_THREAT_XAML), Doc::Hud),
            (m.fill(COCKPIT_CONTACTS_XAML), Doc::Hud),
            (m.fill(COCKPIT_CONSOLE_XAML), Doc::Hud),
            (m.fill(TOUCH_THRUST_XAML), Doc::Controls),
        ];
        if let Some(weapons) = arm.weapons_xaml(&m) {
            docs.push((weapons, Doc::Controls));
        }
        docs
    } else {
        vec![
            (m.fill(HUD_XAML), Doc::Hud),
            (m.fill(PANEL_XAML), Doc::Vessel),
            (m.fill(MAP_XAML), Doc::Map),
        ]
    };
    docs.push((m.fill(EXIT_ORBIT_XAML), Doc::ExitOrbit));
    docs.push((DEATH_XAML.to_string(), Doc::Death));
    if !cockpit {
        // Camera controls belong to the tactical overview.
        docs.push((PAN_ZOOM_XAML.to_string(), Doc::PanZoom));
    }
    if m.touch_controls {
        let topbar = if cockpit { TOUCH_TOPBAR_COCKPIT_XAML } else { TOUCH_TOPBAR_XAML };
        docs.push((m.fill(topbar), Doc::Controls));
        if !cockpit {
            docs.push((m.fill(TOUCH_THRUST_XAML), Doc::Controls));
            if let Some(weapons) = arm.weapons_xaml(&m) {
                docs.push((weapons, Doc::Controls));
            }
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
                // Panels float over the HUD; without an explicit z the
                // HUD text bleeds through (seen on phone portrait).
                world
                    .entity_mut(root)
                    .insert((Visibility::Hidden, GlobalZIndex(20)));
                world.insert_resource(VesselPanel(root));
            }
            Doc::Map => {
                world
                    .entity_mut(root)
                    .insert((Visibility::Hidden, GlobalZIndex(20)));
                world.insert_resource(MapPanel(root));
            }
            Doc::ExitOrbit => {
                // Same key wiring as the clusters, but the button only
                // exists on screen while an orbit is being ridden.
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
                world.entity_mut(root).insert(Visibility::Hidden);
                world.insert_resource(ExitOrbitPanel(root));
            }
            Doc::Death => {
                world
                    .entity_mut(root)
                    .insert((Visibility::Hidden, GlobalZIndex(40)));
                world.insert_resource(DeathPanel(root));
            }
            Doc::PanZoom => {
                // Drag rides window-space math (stick.rs), so nothing in
                // this tree needs — or should swallow — picks.
                ignore_picking_recursive(world, root);
                let names = world
                    .get::<XamlNames>(root)
                    .map(|n| (n.get("pan_knob"), n.get("zoom_knob")))
                    .unwrap_or((None, None));
                if let (Some(pan_knob), Some(zoom_knob)) = names {
                    world.insert_resource(PanZoomUi { pan_knob, zoom_knob });
                }
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
    ships: Query<(&Ship, &crate::SimVel, &NavState, &crate::SimPos)>,
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
    (game, atlas, style_res, raider_q, mut map_rows, lock, arm): (
        Res<crate::GameUniverse>,
        Res<crate::travel::SunAtlas>,
        Res<crate::sim::ShipStyle>,
        Query<
            (
                Entity,
                &crate::SimPos,
                &crate::SimVel,
                Option<&crate::aliens::Dreadnought>,
                Option<&crate::aliens::Elite>,
                Option<&crate::aliens::MineLayer>,
            ),
            With<crate::aliens::AlienShip>,
        >,
        ResMut<crate::travel::MapRows>,
        Res<crate::weapons::TargetLock>,
        Res<crate::solar::SolarArm>,
    ),
    mut sfx: MessageWriter<crate::audio::Sfx>,
    mut was_critical: Local<bool>,
    planner: Res<crate::command::FlightPlanner>,
) {
    let (Some(model), Ok((ship, vel, nav, ship_pos))) = (model, ships.single()) else {
        return;
    };
    let name = |e: bevy::prelude::Entity| {
        bodies
            .get(e)
            .map(|b| b.name.to_uppercase())
            .unwrap_or_else(|_| "?".into())
    };
    let nav_text = if let Some(job) = planner.0.as_ref() {
        model.0.set_plan(job.progress * 100.0);
        format!(">> CALCULATING FLIGHT PLAN {:.0}%", job.progress * 100.0)
    } else if let (Some(target), true) = (hold.target, hold.no_energy) {
        model.0.set_plan(0.0);
        format!(">> {}: INSUFFICIENT ENERGY", name(target))
    } else if let (Some(target), true) = (hold.target, hold.out_of_range) {
        model.0.set_plan(0.0);
        format!(">> {}: OUT OF RANGE", name(target))
    } else {
        model.0.set_plan(0.0);
        match *nav {
            NavState::Free => ">> FREE FLIGHT — CLICK AN ORBIT".into(),
            NavState::Transfer { target, .. } => format!(">> TRANSFER: {}", name(target)),
            NavState::Orbiting { body, speed, .. } => format!(
                ">> ORBIT LOCK: {} · RIDE {:+.1}x {} · [O] EXIT",
                name(body),
                speed.abs(),
                if speed >= 0.0 { "CCW" } else { "CW" }
            ),
        }
    };
    model.0.set_nav(if flash.ttl > 0.0 {
        format!("++ {}", flash.text.to_uppercase())
    } else {
        nav_text
    });
    model.0.set_speed(format!("{:.1} KM/S", vel.0.length() / 1000.0));
    let hdg = (90.0 - vel.0.y.atan2(vel.0.x).to_degrees()).rem_euclid(360.0).round() as i32 % 360;
    model.0.set_heading(format!("<<  HDG {hdg:03}\u{00B0}  >>"));
    // Equality-checked setters: an unchanged readout costs nothing downstream.
    model.0.set_energy((ship.energy / ship.energy_max * 100.0).round());
    model.0.set_shield(ship.shield.round());
    model.0.set_hull((ship.hull / ship.hull_max * 100.0).round());
    model.0.set_energy_text(format!("{:.0}/{:.0}", ship.energy, ship.energy_max));
    model.0.set_shield_text(format!("{:.0}/100", ship.shield));
    model.0.set_hull_text(format!("{:.0}/{:.0}", ship.hull, ship.hull_max));
    model.0.set_score(format!("{}", run.total()));
    model.0.set_best(format!("{}", career.best_run));
    let level = crate::upgrades::pilot_level(run.total());
    model.0.set_level(if run.skill_points > 0 {
        format!("LVL {level} · {} SP", run.skill_points)
    } else {
        format!("LVL {level}")
    });
    model.0.set_salvage(format!("{} CR", run.salvage_balance()));

    model.0.set_style(style_res.label());
    let raiders = raider_q.iter().count();
    let boss_present = raider_q.iter().any(|(_, _, _, boss, _, _)| boss.is_some());
    model.0.set_threat(if boss_present {
        "!! DREADNOUGHT IN-SYSTEM".into()
    } else if raiders > 0 {
        format!("!! {raiders} RAIDER{} IN-SYSTEM", if raiders == 1 { "" } else { "S" })
    } else {
        String::new()
    });

    // Cockpit targeting: nearest contacts with distance, relative speed
    // and closing rate. Closing (+) means it is coming for you. The
    // locked vessel is flagged and drives the console target line.
    let mut target_line = String::new();
    let mut contacts: Vec<(f64, ContactVm)> = Vec::new();
    for (i, (entity, a_pos, a_vel, boss, elite, weaver)) in raider_q.iter().enumerate() {
        let rel = a_pos.0 - ship_pos.0;
        let dist = rel.length();
        let rel_v = a_vel.0 - vel.0;
        let closing = if dist > 1.0 { -(rel.dot(rel_v)) / dist } else { 0.0 };
        let in_laser = dist < 6.0e9;
        let locked = lock.0 == Some(entity);
        let callsign = if boss.is_some() {
            "DREADNOUGHT".to_string()
        } else if weaver.is_some() {
            format!("WEAVER-{} [MINES]", i + 1)
        } else if elite.is_some() {
            format!("ELITE RAIDER-{}", i + 1)
        } else {
            format!("RAIDER-{}", i + 1)
        };
        if locked {
            target_line = format!("TGT LOCK: {callsign} · {:.2} GM", dist / 1.0e9);
        }
        contacts.push((
            dist,
            ContactVm {
                name: format!(
                    "{callsign}{}{}",
                    if locked { "  [LOCKED]" } else { "" },
                    if in_laser { "  [IN RANGE]" } else { "" }
                ),
                data: format!(
                    "DST {:.2} GM · VEL {:.0} KM/S · CLS {:+.0} KM/S",
                    dist / 1.0e9,
                    rel_v.length() / 1000.0,
                    closing / 1000.0
                ),
                color: if locked {
                    "#FF2A9D".into()
                } else if in_laser {
                    "#FF5459".into()
                } else if closing > 0.0 {
                    "#FFB454".into()
                } else {
                    "#5A6472".into()
                },
            },
        ));
    }
    contacts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    // Six, matching the raider pack cap — a full pack fits the stack.
    model.0.set_contacts(contacts.into_iter().map(|(_, c)| c).take(6).collect());
    model.0.set_target(target_line);
    model.0.set_weapon_hints(Armament::of(&ship_upgrades).hints());
    let critical = ship.hull < 25.0;
    if critical && !*was_critical {
        sfx.write(crate::audio::Sfx::Warning);
    }
    *was_critical = critical;
    model.0.set_hull_warn(if critical {
        "!! HULL CRITICAL".into()
    } else {
        String::new()
    });
    model.0.set_arm(match arm.phase {
        crate::solar::ArmPhase::Stowed => String::new(),
        crate::solar::ArmPhase::Deploying => ">> SOLAR ARM EXTENDING — WEAPONS OFFLINE".into(),
        crate::solar::ArmPhase::Deployed => ">> SOLAR ARM DEPLOYED — CHARGING · WEAPONS OFFLINE".into(),
        crate::solar::ArmPhase::Retracting => ">> SOLAR ARM STOWING — WEAPONS OFFLINE".into(),
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

    // Vessel panel rows: each row carries ITS OWN recipe — materials
    // from the stash plus one skill point — and lights up only when both
    // are in hand. No shared currency banner: crafting one row changes
    // that row and the stash chips, nothing else.
    model.0.set_sp_line(format!(
        "SKILL POINTS: {}   ·   MATERIALS FUEL THE FORGE   ·   RANK RESETS ON RESTART",
        run.skill_points
    ));
    // Hull repair sits above the upgrade list: maintenance, not
    // engineering — it spends the salvage credits the HUD shows, no
    // materials, no skill points. What is shown is what can be spent.
    let missing = ship.hull_max - ship.hull;
    if missing < 0.5 {
        model.0.set_repair_line(format!(
            "HULL {:.0}/{:.0} — NOMINAL",
            ship.hull, ship.hull_max
        ));
        model.0.set_repair_color("#5A6472".into());
    } else {
        let cost = crate::upgrades::repair_cost(missing);
        model.0.set_repair_line(format!(
            "HULL {:.0}/{:.0} — PATCH: {} CR ({} CR BANKED)",
            ship.hull,
            ship.hull_max,
            cost,
            run.salvage_balance()
        ));
        let affordable = run.salvage_balance() >= cost;
        model.0.set_repair_color(if affordable { "#7CFFB0" } else { "#FF8A50" }.into());
    }
    let rows: Vec<UpgradeRowVm> = CRAFT_SLOTS
        .iter()
        .enumerate()
        .map(|(i, (slot, label, color))| {
            let next = ship_upgrades.tier(*slot).saturating_add(1);
            let cost = crate::upgrades::material_cost(*slot, next)
                .iter()
                .map(|(e, n)| format!("{n} {}", element_display(*e).0))
                .collect::<Vec<_>>()
                .join(" · ")
                + &format!(" · {} SP", crate::upgrades::CRAFT_POINT_COST);
            let affordable = crate::upgrades::can_afford(*slot, next, &stash, run.skill_points);
            UpgradeRowVm {
                name: (*label).into(),
                tier: format!("TIER {}", ship_upgrades.tier(*slot)),
                cost,
                cost_color: if affordable { "#7CFFB0" } else { "#5A6472" }.into(),
                color: (*color).into(),
                param: i.to_string(),
            }
        })
        .collect();
    model.0.set_rows(rows);

    // Every element gets a chip, zeros included — a recipe's shortfall
    // must be readable as "ICE: 0", not as a missing chip.
    let chips: Vec<StashVm> = {
        use oj_materials::Element as E;
        [E::Iron, E::Titanium, E::Silicon, E::Carbon, E::Ice, E::Uranium, E::Aetherite]
            .iter()
            .map(|e| {
                let n = stash.0.get(e).copied().unwrap_or(0);
                let (sym, color) = element_display(*e);
                StashVm {
                    name: format!("{sym}: {n}"),
                    color: if n > 0 { color.into() } else { "#3A4450".into() },
                }
            })
            .collect()
    };
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
            let cost = crate::travel::jump_cost(
                *dist,
                ship_upgrades.tier(oj_materials::UpgradeSlot::LightDrive),
            );
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
