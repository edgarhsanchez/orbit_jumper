//! Virtual joystick: the on-screen navigation area.
//!
//! A circular pad anchored bottom-left. Drag inside it — finger or mouse —
//! and the displacement becomes a continuous thrust vector: up is
//! prograde, down retrograde, left/right radial in/out, exactly the basis
//! the arrow keys use, but analog. The pad is plain bevy_ui (not XAML):
//! it needs per-frame drag tracking and sub-pixel knob motion, which is
//! pointer-capture work, not layout.

use bevy::input::touch::Touches;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// Pad geometry in logical pixels.
const PAD_MARGIN: f32 = 24.0;
const PAD_SIZE: f32 = 132.0;
const KNOB_SIZE: f32 = 46.0;
/// Landscape phones keep their status column on the left edge; the pad
/// slides right of it. Must agree with ui.rs's landscape thrust margin.
const PAD_LEFT_LANDSCAPE: f32 = 260.0;

/// Left offset for the current window shape.
fn pad_left(window: &Window) -> f32 {
    let (w, h) = (window.width(), window.height());
    if w.min(h) < 500.0 && w > h { PAD_LEFT_LANDSCAPE } else { PAD_MARGIN }
}

/// The stick's current command, consumed by the flight systems.
#[derive(Resource, Default)]
pub struct JoyInput {
    /// Clamped to the unit disc. +y = prograde, +x = radial out.
    pub vec: Vec2,
    /// True while a pointer is captured by the pad.
    pub active: bool,
}

/// Which pointer currently owns the pad.
#[derive(Resource, Default)]
enum JoyPointer {
    #[default]
    None,
    Mouse,
    Touch(u64),
}

#[derive(Component)]
struct JoyBase;

#[derive(Component)]
struct JoyKnob;

pub struct StickPlugin;

impl Plugin for StickPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<JoyInput>()
            .init_resource::<JoyPointer>()
            .add_systems(Startup, spawn_pad)
            .add_systems(PreUpdate, drive_pad.after(bevy::ui::UiSystems::Focus))
            .add_systems(Update, (move_knob, place_pad));
    }
}

fn spawn_pad(mut commands: Commands) {
    commands
        .spawn((
            JoyBase,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(PAD_MARGIN),
                bottom: Val::Px(PAD_MARGIN),
                width: Val::Px(PAD_SIZE),
                height: Val::Px(PAD_SIZE),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Percent(50.0)),
                ..default()
            },
            BorderColor::all(Color::srgba(0.0, 0.9, 0.83, 0.55)),
            BackgroundColor(Color::srgba(0.02, 0.08, 0.09, 0.55)),
            // The pad reads raw window pointers itself; keep bevy_ui
            // picking from routing its presses anywhere else.
            bevy::picking::Pickable::IGNORE,
            GlobalZIndex(30),
        ))
        .with_children(|base| {
            base.spawn((
                JoyKnob,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px((PAD_SIZE - KNOB_SIZE) / 2.0),
                    top: Val::Px((PAD_SIZE - KNOB_SIZE) / 2.0),
                    width: Val::Px(KNOB_SIZE),
                    height: Val::Px(KNOB_SIZE),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Percent(50.0)),
                    ..default()
                },
                BorderColor::all(Color::srgba(0.0, 1.0, 0.83, 0.9)),
                BackgroundColor(Color::srgba(0.0, 0.55, 0.5, 0.55)),
                bevy::picking::Pickable::IGNORE,
            ));
            // NAV label, so the area reads as a control, not decoration.
            base.spawn((
                Text::new("NAV"),
                TextFont { font_size: bevy::text::FontSize::Px(9.0), ..default() },
                TextColor(Color::srgba(0.24, 0.42, 0.4, 1.0)),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(PAD_SIZE / 2.0 - 10.0),
                    top: Val::Px(-14.0),
                    ..default()
                },
                bevy::picking::Pickable::IGNORE,
            ));
        });
}

/// Pad center in logical window coordinates (y down, like cursor space).
fn pad_center(window: &Window) -> Vec2 {
    Vec2::new(
        pad_left(window) + PAD_SIZE / 2.0,
        window.height() - PAD_MARGIN - PAD_SIZE / 2.0,
    )
}

/// Track window shape: the pad hops to its per-orientation anchor.
fn place_pad(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut pads: Query<&mut Node, With<JoyBase>>,
) {
    let (Ok(window), Ok(mut node)) = (windows.single(), pads.single_mut()) else { return };
    let left = Val::Px(pad_left(window));
    if node.left != left {
        node.left = left;
    }
}

fn drive_pad(
    windows: Query<&Window, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    mut joy: ResMut<JoyInput>,
    mut owner: ResMut<JoyPointer>,
) {
    let Ok(window) = windows.single() else { return };
    let center = pad_center(window);
    let radius = PAD_SIZE / 2.0;
    // Displacement from a window-space position, y flipped so up is +y.
    let to_vec = |pos: Vec2| {
        let d = (pos - center) / radius;
        let v = Vec2::new(d.x, -d.y);
        if v.length() > 1.0 { v.normalize() } else { v }
    };

    // Claim: a pointer that goes down inside the pad owns it until release.
    if matches!(*owner, JoyPointer::None) {
        if mouse.just_pressed(MouseButton::Left)
            && let Some(pos) = window.cursor_position()
            && pos.distance(center) <= radius
        {
            *owner = JoyPointer::Mouse;
        } else {
            for touch in touches.iter_just_pressed() {
                if touch.position().distance(center) <= radius {
                    *owner = JoyPointer::Touch(touch.id());
                    break;
                }
            }
        }
    }

    match *owner {
        JoyPointer::Mouse => {
            if mouse.pressed(MouseButton::Left) {
                joy.vec = window.cursor_position().map(to_vec).unwrap_or(joy.vec);
                joy.active = true;
            } else {
                *owner = JoyPointer::None;
                joy.vec = Vec2::ZERO;
                joy.active = false;
            }
        }
        JoyPointer::Touch(id) => {
            if let Some(touch) = touches.iter().find(|t| t.id() == id) {
                joy.vec = to_vec(touch.position());
                joy.active = true;
            } else {
                *owner = JoyPointer::None;
                joy.vec = Vec2::ZERO;
                joy.active = false;
            }
        }
        JoyPointer::None => {
            joy.vec = Vec2::ZERO;
            joy.active = false;
        }
    }
}

fn move_knob(joy: Res<JoyInput>, mut knobs: Query<&mut Node, With<JoyKnob>>) {
    let Ok(mut node) = knobs.single_mut() else { return };
    let reach = (PAD_SIZE - KNOB_SIZE) / 2.0;
    node.left = Val::Px((PAD_SIZE - KNOB_SIZE) / 2.0 + joy.vec.x * reach);
    node.top = Val::Px((PAD_SIZE - KNOB_SIZE) / 2.0 - joy.vec.y * reach);
}
