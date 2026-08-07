//! Sound. Everything is procedurally synthesized offline (see the repo
//! history for the generator) and EMBEDDED in the binary — the web build
//! ships no asset directory, so bytes-in-binary is what works everywhere.
//!
//! Three layers:
//! - the space drone: a 64-second seamless ambient loop, quiet and vast,
//!   always on (browsers gate autoplay behind the first gesture; cpal
//!   resumes the context on it)
//! - the engine hum: loops muted and fades up while the pilot burns
//! - one-shot [`Sfx`] messages sent by gameplay systems

use bevy::audio::{
    AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, PlaybackSettings, Volume,
};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::command::NavState;
use crate::sim::Ship;

const DRONE_OGG: &[u8] = include_bytes!("../audio/space_drone.ogg");
const ENGINE_OGG: &[u8] = include_bytes!("../audio/engine_loop.ogg");
const LASER_OGG: &[u8] = include_bytes!("../audio/laser.ogg");
const MISSILE_OGG: &[u8] = include_bytes!("../audio/missile.ogg");
const EXPLOSION_OGG: &[u8] = include_bytes!("../audio/explosion.ogg");
const SHIELD_OGG: &[u8] = include_bytes!("../audio/shield_hit.ogg");
const HULL_OGG: &[u8] = include_bytes!("../audio/hull_hit.ogg");
const CLICK_OGG: &[u8] = include_bytes!("../audio/click.ogg");
const ORBIT_OGG: &[u8] = include_bytes!("../audio/orbit_lock.ogg");
const SALVAGE_OGG: &[u8] = include_bytes!("../audio/salvage.ogg");
const SOLAR_OGG: &[u8] = include_bytes!("../audio/solar_arm.ogg");
const WARNING_OGG: &[u8] = include_bytes!("../audio/warning.ogg");

/// A one-shot sound request. Any system may write these.
#[derive(Message, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Sfx {
    Laser,
    Missile,
    Explosion,
    ShieldHit,
    HullHit,
    Click,
    OrbitLock,
    Salvage,
    SolarArm,
    Warning,
}

impl Sfx {
    fn volume(self) -> f32 {
        match self {
            Sfx::Laser => 0.5,
            Sfx::Missile => 0.55,
            Sfx::Explosion => 0.8,
            Sfx::ShieldHit => 0.6,
            Sfx::HullHit => 0.7,
            Sfx::Click => 0.35,
            Sfx::OrbitLock => 0.5,
            Sfx::Salvage => 0.45,
            Sfx::SolarArm => 0.5,
            Sfx::Warning => 0.6,
        }
    }
}

#[derive(Resource)]
struct SoundBank {
    sfx: HashMap<Sfx, Handle<AudioSource>>,
}

/// The muted-until-burning engine loop.
#[derive(Component)]
struct EngineHum;

/// A track that must play forever. rodio 0.22's `repeat_infinite` DIES
/// at the loop seam on our vorbis streams (pinned by the test below:
/// "source ended inside pass 1") — in the shipped game the space drone
/// played once and went silent ~62 s in. So loops are self-hosted:
/// each track plays as a one-shot and is respawned the moment its sink
/// drains. The tracks carry a crossfaded loop head, which doubles as a
/// graceful mask for the one-frame seam.
#[derive(Component)]
struct LoopTrack {
    source: Handle<AudioSource>,
}

pub struct GameAudioPlugin;

impl Plugin for GameAudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Sfx>()
            .add_systems(Startup, setup_audio)
            .add_systems(Update, (play_sfx, drive_engine_hum, keep_loops_alive));
    }
}

fn setup_audio(
    mut commands: Commands,
    mut sources: ResMut<Assets<AudioSource>>,
) {
    let mut add = |bytes: &'static [u8]| {
        sources.add(AudioSource { bytes: bytes.into() })
    };
    // The vastness: always on, under everything. ONCE, not LOOP — the
    // LoopTrack respawn owns eternity (see LoopTrack for why).
    let drone = add(DRONE_OGG);
    commands.spawn((
        LoopTrack { source: drone.clone() },
        AudioPlayer::new(drone),
        PlaybackSettings::ONCE.with_volume(Volume::Linear(0.55)),
    ));
    // The engine: same self-hosted loop, silent until a burn fades it up.
    let engine = add(ENGINE_OGG);
    commands.spawn((
        EngineHum,
        LoopTrack { source: engine.clone() },
        AudioPlayer::new(engine),
        PlaybackSettings::ONCE.with_volume(Volume::Linear(0.0)),
    ));
    let sfx = [
        (Sfx::Laser, LASER_OGG),
        (Sfx::Missile, MISSILE_OGG),
        (Sfx::Explosion, EXPLOSION_OGG),
        (Sfx::ShieldHit, SHIELD_OGG),
        (Sfx::HullHit, HULL_OGG),
        (Sfx::Click, CLICK_OGG),
        (Sfx::OrbitLock, ORBIT_OGG),
        (Sfx::Salvage, SALVAGE_OGG),
        (Sfx::SolarArm, SOLAR_OGG),
        (Sfx::Warning, WARNING_OGG),
    ]
    .into_iter()
    .map(|(k, bytes)| (k, add(bytes)))
    .collect();
    commands.insert_resource(SoundBank { sfx });
}

/// The self-hosted loop: the moment a LoopTrack's sink drains, respawn
/// the same source at the same volume. One frame of seam, masked by the
/// tracks' crossfaded loop heads.
fn keep_loops_alive(
    tracks: Query<(Entity, &LoopTrack, Option<&EngineHum>, &AudioSink)>,
    mut commands: Commands,
) {
    for (entity, track, hum, sink) in &tracks {
        if !sink.empty() {
            continue;
        }
        let volume = sink.volume();
        commands.entity(entity).despawn();
        let mut fresh = commands.spawn((
            LoopTrack { source: track.source.clone() },
            AudioPlayer::new(track.source.clone()),
            PlaybackSettings::ONCE.with_volume(volume),
        ));
        if hum.is_some() {
            fresh.insert(EngineHum);
        }
    }
}

/// Fire every requested one-shot, capped per frame so a ten-kill volley
/// is loud but not a hundred stacked decoders.
fn play_sfx(
    mut requests: MessageReader<Sfx>,
    bank: Res<SoundBank>,
    mut commands: Commands,
) {
    let mut counts: HashMap<Sfx, u32> = HashMap::default();
    for &sfx in requests.read() {
        let n = counts.entry(sfx).or_default();
        *n += 1;
        if *n > 2 {
            continue;
        }
        if let Some(handle) = bank.sfx.get(&sfx) {
            commands.spawn((
                AudioPlayer::new(handle.clone()),
                PlaybackSettings::DESPAWN.with_volume(Volume::Linear(sfx.volume())),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The looping tracks must decode as full, audible passes — the
    /// LoopTrack respawn replays them end to end, so a truncated or
    /// silent decode would be a silent game. (rodio's `repeat_infinite`
    /// is deliberately NOT under test: it ends after one pass on these
    /// vorbis streams — "source ended inside pass 1" — which is exactly
    /// the went-silent-after-a-minute bug the respawn loop replaced it
    /// over.)
    #[test]
    fn loop_tracks_decode_full_audible_passes() {
        use rodio::Source;
        for (name, bytes, min_secs) in
            [("drone", DRONE_OGG, 60.0f64), ("engine", ENGINE_OGG, 1.5)]
        {
            let cursor = std::io::Cursor::new(bytes);
            let decoder = rodio::Decoder::new(cursor)
                .unwrap_or_else(|e| panic!("{name} failed to decode: {e}"));
            let channels = decoder.channels().get() as f64;
            let rate = decoder.sample_rate().get() as f64;
            let mut n = 0u64;
            let mut energy = 0.0f64;
            for s in decoder {
                n += 1;
                energy += (s as f64).abs();
            }
            let secs = n as f64 / (channels * rate);
            assert!(secs > min_secs, "{name} pass is only {secs:.1}s");
            let mean = energy / n as f64;
            assert!(mean > 0.005, "{name} decodes silent (mean |sample| {mean:.6})");
        }
    }

    /// Every embedded one-shot must decode — a corrupt ogg would panic
    /// rodio at spawn time in the shipped game.
    #[test]
    fn every_embedded_sound_decodes() {
        for (name, bytes) in [
            ("drone", DRONE_OGG),
            ("engine", ENGINE_OGG),
            ("laser", LASER_OGG),
            ("missile", MISSILE_OGG),
            ("explosion", EXPLOSION_OGG),
            ("shield", SHIELD_OGG),
            ("hull", HULL_OGG),
            ("click", CLICK_OGG),
            ("orbit", ORBIT_OGG),
            ("salvage", SALVAGE_OGG),
            ("solar", SOLAR_OGG),
            ("warning", WARNING_OGG),
        ] {
            let cursor = std::io::Cursor::new(bytes);
            let decoder = rodio::Decoder::new(cursor)
                .unwrap_or_else(|e| panic!("{name} failed to decode: {e}"));
            let n = decoder.count();
            assert!(n > 400, "{name} decoded only {n} samples");
        }
    }
}

/// The hum tracks the burn: fades up when thrusting under manual flight,
/// down to silence when coasting, riding, or dry.
fn drive_engine_hum(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    joy: Res<crate::stick::JoyInput>,
    ships: Query<(&Ship, &NavState)>,
    mut hums: Query<&mut AudioSink, With<EngineHum>>,
) {
    let Ok(mut sink) = hums.single_mut() else { return };
    let burning = ships.single().is_ok_and(|(ship, nav)| {
        ship.energy > 0.0
            && !matches!(nav, NavState::Orbiting { .. })
            && (joy.active
                || keys.any_pressed([
                    KeyCode::ArrowUp,
                    KeyCode::ArrowDown,
                    KeyCode::ArrowLeft,
                    KeyCode::ArrowRight,
                    KeyCode::KeyE,
                    KeyCode::KeyQ,
                ]))
    });
    let target = if burning { 0.45 } else { 0.0 };
    let current = sink.volume().to_linear();
    let step = (target - current).clamp(-3.0, 3.0) * (time.delta_secs() * 6.0).min(1.0);
    sink.set_volume(Volume::Linear(current + step));
}
