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
            .init_resource::<CamPointer>()
            .add_systems(Startup, spawn_pad)
            .add_systems(
                PreUpdate,
                (drive_pad, drive_cam_controls).after(bevy::ui::UiSystems::Focus),
            )
            .add_systems(Update, (move_knob, place_pad, move_cam_knobs));
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

// ---------------------------------------------------------------------------
// Camera controls: the yellow pan pad + darker-blue zoom slider.
// Visuals are XAML (ui.rs PAN_ZOOM_XAML, Stitch terminal styling); the
// drag needs pointer capture, so it rides the same window-space math as
// the NAV stick. Geometry here MUST agree with the markup.
// ---------------------------------------------------------------------------

/// Pan pad geometry, logical px (matches PAN_ZOOM_XAML).
const PAN_MARGIN_RIGHT: f32 = 110.0;
const PAN_MARGIN_BOTTOM: f32 = 12.0;
const PAN_SIZE: f32 = 84.0;
const PAN_KNOB: f32 = 30.0;
/// Zoom slider geometry: sits GAP left of the pan pad.
const ZOOMBAR_W: f32 = 14.0;
const ZOOMBAR_H: f32 = 96.0;
const ZOOMBAR_GAP: f32 = 8.0;
const ZOOM_KNOB_H: f32 = 12.0;
/// Slider range; the dynamic big-orbit ceiling stays on wheel/keys.
const ZOOM_MIN: f32 = 160.0;
const ZOOM_MAX: f32 = 4000.0;

/// Which pointer owns which camera control.
#[derive(Resource, Default)]
enum CamPointer {
    #[default]
    None,
    PanMouse,
    PanTouch(u64),
    ZoomMouse,
    ZoomTouch(u64),
}

fn pan_center(window: &Window) -> Vec2 {
    Vec2::new(
        window.width() - PAN_MARGIN_RIGHT - PAN_SIZE / 2.0,
        window.height() - PAN_MARGIN_BOTTOM - PAN_SIZE / 2.0,
    )
}

/// The zoom track's rect: (min, max) corners in window space.
fn zoom_rect(window: &Window) -> (Vec2, Vec2) {
    let right = window.width() - PAN_MARGIN_RIGHT - PAN_SIZE - ZOOMBAR_GAP;
    let bottom = window.height() - PAN_MARGIN_BOTTOM;
    (Vec2::new(right - ZOOMBAR_W, bottom - ZOOMBAR_H), Vec2::new(right, bottom))
}

/// Normalized zoom position: 0 = nearest (bottom), 1 = farthest (top).
fn zoom_t(zoom: f32) -> f32 {
    ((zoom / ZOOM_MIN).ln() / (ZOOM_MAX / ZOOM_MIN).ln()).clamp(0.0, 1.0)
}

/// Claim-and-track for both camera controls: dragging the pan pad
/// orbits the view around the ship (x swings azimuth, y tilts), and
/// dragging the slider sets zoom on a log scale — push UP to pull back.
#[allow(clippy::too_many_arguments)]
fn drive_cam_controls(
    time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    view: Res<crate::sim::ViewMode>,
    mut owner: ResMut<CamPointer>,
    mut rig: ResMut<crate::sim::CameraRig>,
) {
    if *view != crate::sim::ViewMode::Tactical {
        *owner = CamPointer::None;
        return;
    }
    let Ok(window) = windows.single() else { return };
    let center = pan_center(window);
    let radius = PAN_SIZE / 2.0;
    let (zmin, zmax) = zoom_rect(window);
    let in_zoom =
        |p: Vec2| p.x >= zmin.x - 6.0 && p.x <= zmax.x + 6.0 && p.y >= zmin.y && p.y <= zmax.y;

    if matches!(*owner, CamPointer::None) {
        if mouse.just_pressed(MouseButton::Left) {
            if let Some(pos) = window.cursor_position() {
                if pos.distance(center) <= radius {
                    *owner = CamPointer::PanMouse;
                } else if in_zoom(pos) {
                    *owner = CamPointer::ZoomMouse;
                }
            }
        } else {
            for touch in touches.iter_just_pressed() {
                let pos = touch.position();
                if pos.distance(center) <= radius {
                    *owner = CamPointer::PanTouch(touch.id());
                    break;
                } else if in_zoom(pos) {
                    *owner = CamPointer::ZoomTouch(touch.id());
                    break;
                }
            }
        }
    }

    let pan_vec = |pos: Vec2| {
        let d = (pos - center) / radius;
        if d.length() > 1.0 { d.normalize() } else { d }
    };
    let apply_pan = |rig: &mut crate::sim::CameraRig, v: Vec2, dt: f32| {
        rig.yaw += v.x * dt * 1.8;
        rig.pitch = (rig.pitch - v.y * dt * 1.1).clamp(0.30, 1.50);
    };
    let apply_zoom = |rig: &mut crate::sim::CameraRig, pos: Vec2| {
        let t = ((zmax.y - pos.y) / ZOOMBAR_H).clamp(0.0, 1.0);
        rig.zoom = ZOOM_MIN * (ZOOM_MAX / ZOOM_MIN).powf(t);
    };
    let dt = time.delta_secs();
    match *owner {
        CamPointer::PanMouse => {
            if mouse.pressed(MouseButton::Left) {
                if let Some(pos) = window.cursor_position() {
                    apply_pan(&mut rig, pan_vec(pos), dt);
                }
            } else {
                *owner = CamPointer::None;
            }
        }
        CamPointer::PanTouch(id) => {
            if let Some(touch) = touches.iter().find(|t| t.id() == id) {
                apply_pan(&mut rig, pan_vec(touch.position()), dt);
            } else {
                *owner = CamPointer::None;
            }
        }
        CamPointer::ZoomMouse => {
            if mouse.pressed(MouseButton::Left) {
                if let Some(pos) = window.cursor_position() {
                    apply_zoom(&mut rig, pos);
                }
            } else {
                *owner = CamPointer::None;
            }
        }
        CamPointer::ZoomTouch(id) => {
            if let Some(touch) = touches.iter().find(|t| t.id() == id) {
                apply_zoom(&mut rig, touch.position());
            } else {
                *owner = CamPointer::None;
            }
        }
        CamPointer::None => {}
    }
}

/// Reflect state back into the XAML knobs: the pan knob leans with the
/// active drag; the zoom knob rides wherever the zoom is, whatever set
/// it (slider, wheel, or keys).
fn move_cam_knobs(
    ui: Option<Res<crate::ui::PanZoomUi>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    owner: Res<CamPointer>,
    rig: Res<crate::sim::CameraRig>,
    mut nodes: Query<&mut Node>,
) {
    let Some(ui) = ui else { return };
    let Ok(window) = windows.single() else { return };
    // Pan knob: lean toward the pointer while owned, else recenter.
    let lean = match *owner {
        CamPointer::PanMouse if mouse.pressed(MouseButton::Left) => window
            .cursor_position()
            .map(|p| {
                let d = (p - pan_center(window)) / (PAN_SIZE / 2.0);
                if d.length() > 1.0 { d.normalize() } else { d }
            })
            .unwrap_or(Vec2::ZERO),
        CamPointer::PanTouch(id) => touches
            .iter()
            .find(|t| t.id() == id)
            .map(|t| {
                let d = (t.position() - pan_center(window)) / (PAN_SIZE / 2.0);
                if d.length() > 1.0 { d.normalize() } else { d }
            })
            .unwrap_or(Vec2::ZERO),
        _ => Vec2::ZERO,
    };
    let max_lean = (PAN_SIZE - PAN_KNOB) / 2.0;
    if let Ok(mut node) = nodes.get_mut(ui.pan_knob) {
        node.left = Val::Px(lean.x * max_lean);
        node.top = Val::Px(lean.y * max_lean);
    }
    // Zoom knob: top of the track = far, bottom = near.
    if let Ok(mut node) = nodes.get_mut(ui.zoom_knob) {
        let t = zoom_t(rig.zoom);
        node.top = Val::Px((1.0 - t) * (ZOOMBAR_H - ZOOM_KNOB_H - 4.0) + 1.0);
    }
}
