//! Command line options and unattended screenshots (`--shot out.png`), used to check the
//! renderer without a person at the keyboard.

use std::path::PathBuf;

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};

/// `som-game [--shot FILE] [--cam DIST YAW PITCH] [--start X Z YAW] [--burst FROM STEP COUNT]
///           [--hold KEY FROM TO]...`
///
/// `--hold W 1 3.5` presses W from 1 s to 3.5 s after start (keys: letters, Space, Shift, Ctrl), for
/// unattended checks of movement.

#[derive(Resource, Default, Clone)]
pub struct Options {
    /// Scripted key presses: key, from (s), to (s).
    pub hold: Vec<(KeyCode, f32, f32)>,
    /// Save one frame here, then quit.
    pub shot: Option<PathBuf>,
    /// With `--shot`: save `COUNT` frames, one every `STEP` seconds from `FROM`, as `<name>_<n>.png`.
    pub burst: Option<(f32, f32, u32)>,
    /// Starting orbit camera: distance (m), yaw and pitch (degrees).
    pub camera: Option<(f32, f32, f32)>,
    /// Where the player starts: x, z (m) and heading (degrees).
    pub start: Option<(f32, f32, f32)>,
}

impl Options {
    pub fn from_args() -> Self {
        let mut options = Self::default();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--hold" => {
                    let key = args.next().and_then(|k| parse_key(&k));
                    let mut time = || args.next().and_then(|v| v.parse::<f32>().ok());
                    if let (Some(key), Some(from), Some(to)) = (key, time(), time()) {
                        options.hold.push((key, from, to));
                    }
                }
                "--burst" => {
                    let mut next = || args.next().and_then(|v| v.parse::<f32>().ok());
                    if let (Some(from), Some(step), Some(count)) = (next(), next(), next()) {
                        options.burst = Some((from, step, count as u32));
                    }
                }
                "--shot" => options.shot = args.next().map(PathBuf::from),
                "--start" => {
                    let mut value = || args.next().and_then(|v| v.parse::<f32>().ok());
                    if let (Some(x), Some(z), Some(yaw)) = (value(), value(), value()) {
                        options.start = Some((x, z, yaw));
                    }
                }
                "--cam" => {
                    let mut value = || args.next().and_then(|v| v.parse::<f32>().ok());
                    if let (Some(dist), Some(yaw), Some(pitch)) = (value(), value(), value()) {
                        options.camera = Some((dist, yaw, pitch));
                    }
                }
                other => warn!("ignoring unknown argument {other}"),
            }
        }
        options
    }
}

fn parse_key(name: &str) -> Option<KeyCode> {
    Some(match name.to_ascii_lowercase().as_str() {
        "w" => KeyCode::KeyW,
        "a" => KeyCode::KeyA,
        "s" => KeyCode::KeyS,
        "d" => KeyCode::KeyD,
        "q" => KeyCode::KeyQ,
        "e" => KeyCode::KeyE,
        "f" => KeyCode::KeyF,
        "l" => KeyCode::KeyL,
        "h" => KeyCode::KeyH,
        "c" => KeyCode::KeyC,
        "t" => KeyCode::KeyT,
        "space" => KeyCode::Space,
        "shift" => KeyCode::ShiftLeft,
        "ctrl" => KeyCode::ControlLeft,
        _ => return None,
    })
}

impl Options {
    /// Whether `key` is held, by the keyboard or by a `--hold` script.
    pub fn pressed(&self, keys: &ButtonInput<KeyCode>, time: &Time, key: KeyCode) -> bool {
        let now = time.elapsed_secs();
        keys.pressed(key) || self.hold.iter().any(|&(k, from, to)| k == key && (from..to).contains(&now))
    }
}

pub struct ShotPlugin;

impl Plugin for ShotPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, take_shot);
    }
}

/// Seconds to let pipelines compile and the scene settle before capturing, and frames to
/// let the file be written after.
const CAPTURE_AFTER_SECS: f32 = 4.0;
const FRAMES_AFTER_CAPTURE: u32 = 20;

#[derive(Default)]
struct ShotState {
    frame: u32,
    shots: u32,
    requested: bool,
    captured_at: Option<u32>,
}

#[derive(Resource, Default)]
struct ShotCaptured(bool);

fn take_shot(
    mut commands: Commands,
    options: Res<Options>,
    time: Res<Time>,
    captured: Option<Res<ShotCaptured>>,
    mut state: Local<ShotState>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(path) = &options.shot else { return };
    state.frame += 1;
    if let Some((from, step, count)) = options.burst {
        // One frame every `step` seconds; quit shortly after the last one has been written.
        if state.shots < count && time.elapsed_secs() >= from + step * state.shots as f32 {
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let file = path.with_file_name(format!("{stem}_{:02}.png", state.shots));
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(file));
            state.shots += 1;
            state.captured_at = Some(state.frame);
        }
        if state.shots >= count && state.captured_at.is_some_and(|at| state.frame >= at + FRAMES_AFTER_CAPTURE) {
            exit.write(AppExit::Success);
        }
        return;
    }
    let capture_at = std::env::var("SOM_SHOT_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(CAPTURE_AFTER_SECS);
    if !state.requested && time.elapsed_secs() >= capture_at {
        state.requested = true;
        commands.init_resource::<ShotCaptured>();
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()))
            .observe(|_: On<ScreenshotCaptured>, mut captured: ResMut<ShotCaptured>| captured.0 = true);
    }
    // Quit only once the frame has come back from the GPU and had time to reach the disk.
    if state.captured_at.is_none() && captured.is_some_and(|c| c.0) {
        state.captured_at = Some(state.frame);
    }
    if state.captured_at.is_some_and(|at| state.frame >= at + FRAMES_AFTER_CAPTURE) {
        exit.write(AppExit::Success);
    }
}
