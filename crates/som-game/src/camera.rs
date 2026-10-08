//! Third-person follow camera: mouse look around the player on a spring arm.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use som_formats::bvrgraph::CameraProfile;

use crate::player::Player;
use crate::shot::Options;

pub struct FollowCameraPlugin;

impl Plugin for FollowCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera).add_systems(Update, (grab_cursor, follow_camera).chain());
    }
}

#[derive(Component)]
pub struct FollowCamera {
    /// Heading of the camera about Y; 0 looks toward -Z.
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    /// Smoothed point the camera looks at.
    focus: Vec3,
    /// The numbers of the camera state in force (blended toward the player's current one), and seconds since the mouse last moved the view.
    profile: CameraProfile,
    idle: f32,
}

/// Height of the point the camera orbits (m), about Talion's shoulders.
const FOCUS_HEIGHT: f32 = 1.55;
/// Sideways shoulder offset (m) so the character sits left of centre.
const SHOULDER: f32 = 0.35;

fn spawn_camera(mut commands: Commands, options: Res<Options>) {
    let (distance, yaw, pitch) = options.camera.unwrap_or((3.6, 0.0, 14.0));
    commands.spawn((
        Camera3d::default(),
        FollowCamera {
            yaw: yaw.to_radians(),
            pitch: pitch.to_radians(),
            distance,
            focus: Vec3::new(0.0, FOCUS_HEIGHT, 0.0),
            profile: CameraProfile { dist_min: 400.0, dist_max: 400.0, pitch_default: 5.0, pitch_smooth: 1.0, pitch_limits: (-65.0, 65.0), yaw_smooth: 0.6, yaw_max_speed: 80.0, yaw_velocity: 50.0, yaw_thresh: 8.0, fov: 40.0, ..default() },
            idle: 0.0,
        },
        Transform::default(),
    ));
}

/// Capture the mouse for looking around; Esc gives it back, a click takes it again.
fn grab_cursor(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    options: Res<Options>,
    mut window: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut started: Local<bool>,
) {
    let Ok(mut cursor) = window.single_mut() else { return };
    let grab = |cursor: &mut CursorOptions, on: bool| {
        cursor.grab_mode = if on { CursorGrabMode::Locked } else { CursorGrabMode::None };
        cursor.visible = !on;
    };
    if !*started {
        *started = true;
        // Unattended screenshots must not trap the user's mouse.
        grab(&mut cursor, options.shot.is_none() && options.hold.is_empty());
    }
    if keys.just_pressed(KeyCode::Escape) {
        grab(&mut cursor, false);
    } else if buttons.just_pressed(MouseButton::Left) && options.shot.is_none() {
        grab(&mut cursor, true);
    }
}

/// Height of the bone a camera state follows (m above the feet): the head for `Null_Head_OverrideY`, the chest for `Null_Chest_Stl`.
/// (Approximate: the real bones' heights move with the animation.)
fn focus_height(bone: &str) -> f32 {
    match bone {
        "Null_Head_OverrideY" => 1.65,
        "Null_Chest_Stl" => 1.35,
        "Hanging_Collision" => 1.2,
        _ => FOCUS_HEIGHT,
    }
}

fn blend_profile(cur: &mut CameraProfile, to: &CameraProfile, k: f32) {
    let l = |a: &mut f32, b: f32| *a += (b - *a) * k;
    l(&mut cur.dist_min, to.dist_min);
    l(&mut cur.dist_max, to.dist_max);
    l(&mut cur.pitch_default, to.pitch_default);
    l(&mut cur.pitch_smooth, to.pitch_smooth);
    l(&mut cur.pitch_limits.0, to.pitch_limits.0);
    l(&mut cur.pitch_limits.1, to.pitch_limits.1);
    l(&mut cur.yaw_smooth, to.yaw_smooth);
    l(&mut cur.yaw_max_speed, to.yaw_max_speed);
    l(&mut cur.yaw_velocity, to.yaw_velocity);
    l(&mut cur.yaw_thresh, to.yaw_thresh);
    l(&mut cur.fov, to.fov);
    for i in 0..3 {
        l(&mut cur.pos_smooth[i], to.pos_smooth[i]);
    }
    cur.focus = to.focus.clone();
}

/// How long the view stays as the mouse left it before the camera's own pitch and yaw take over (`CameraPitch.InitTime`, `CameraYaw` hold).
const MOUSE_HOLD: f32 = 1.0;
/// `Value:CameraRoot_PosSmoothTimeY`: the height of the followed point is smoothed even where the horizontal is not.
const ROOT_Y_SMOOTH: f32 = 0.2;

#[allow(clippy::too_many_arguments)]
pub fn follow_camera(
    time: Res<Time>,
    options: Res<Options>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    player: Option<Res<Player>>,
    world: Option<Res<crate::world::WorldGeometry>>,
    window: Query<&CursorOptions, With<PrimaryWindow>>,
    mut cameras: Query<(&mut FollowCamera, &mut Transform, &mut Projection)>,
) {
    let looking = window.single().is_ok_and(|c| c.grab_mode != CursorGrabMode::None);
    let scripted = options.shot.is_some() || !options.hold.is_empty();
    let dt = time.delta_secs().min(0.05);
    for (mut cam, mut transform, mut projection) in &mut cameras {
        if let Some(target) = player.as_ref().and_then(|p| p.camera_profile()) {
            let mut cur = cam.profile.clone();
            blend_profile(&mut cur, &target, 1.0 - (-3.0 * dt).exp());
            cam.profile = cur;
        }
        let profile = cam.profile.clone();
        let moved = looking && motion.delta.length_squared() > 0.0;
        if looking {
            cam.yaw -= motion.delta.x * 0.0025;
            cam.pitch = (cam.pitch + motion.delta.y * 0.0025).clamp(profile.pitch_limits.0.to_radians().min(-0.35), profile.pitch_limits.1.to_radians().max(0.35));
        }
        cam.idle = if moved { 0.0 } else { cam.idle + dt };
        // The camera's own pitch and yaw: with the view left alone it settles at the state's resting pitch, and turns behind him while he runs.
        if !scripted && cam.idle > MOUSE_HOLD {
            let k = 1.0 - (-dt / profile.pitch_smooth.max(0.05)).exp();
            let want = profile.pitch_default.to_radians();
            cam.pitch += (want - cam.pitch) * k;
            if let Some(p) = player.as_ref() {
                // Behind him: the camera looks along his heading (his yaw 0 faces -Z, as the camera's does).
                if p.speed() * 100.0 > profile.yaw_velocity {
                    let diff = (p.yaw - cam.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                    if diff.abs().to_degrees() > profile.yaw_thresh {
                        let rate = (diff / profile.yaw_smooth.max(0.05)).clamp(-profile.yaw_max_speed.to_radians(), profile.yaw_max_speed.to_radians());
                        cam.yaw += rate * dt;
                    }
                }
            }
        }
        let fixed = options.camera.map(|c| c.0);
        cam.distance = fixed.unwrap_or(((profile.dist_min + profile.dist_max) * 0.5 * 0.01 * (1.0 - scroll.delta.y * 0.1)).clamp(1.2, 12.0));
        if let Projection::Perspective(p) = &mut *projection {
            p.fov = profile.fov.to_radians();
        }

        let rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, -cam.pitch, 0.0);
        let pos = player.as_ref().map_or(Vec3::ZERO, |p| p.pos);
        let height = focus_height(&profile.focus);
        let target = pos + Vec3::Y * height + rotation * Vec3::X * SHOULDER;
        // Follow the root with the state's smoothing times (`CameraSetRoot.PosSmoothTime`; the height always has a little).
        let smooth = |t: f32| if t <= 0.001 { 1.0 } else { 1.0 - (-dt / t).exp() };
        let f = cam.focus;
        cam.focus = Vec3::new(
            f.x + (target.x - f.x) * smooth(profile.pos_smooth[0]),
            f.y + (target.y - f.y) * smooth(profile.pos_smooth[1].max(ROOT_Y_SMOOTH)),
            f.z + (target.z - f.z) * smooth(profile.pos_smooth[2]),
        );

        // The arm shortens rather than pass through a wall.
        let back = rotation * Vec3::Z;
        let reach = world
            .as_ref()
            .and_then(|w| w.raycast(cam.focus, back, cam.distance + 0.3))
            .map_or(cam.distance, |(hit, _)| (hit - 0.3).max(0.4));
        let mut position = cam.focus + back * reach;
        position.y = position.y.max(0.15);
        transform.translation = position;
        transform.rotation = rotation;
    }
}
