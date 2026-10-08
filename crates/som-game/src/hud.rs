//! The in-game HUD, laid out as the game's (measured on 1280x720 captures; everything scales with the window height):
//!
//! * bottom left, the round minimap with the **health arc** on its left rim (red, draining from the top), the focus arc at
//!   its lower right (teal segments) and the arrow count above it;
//! * over each orc in the fight, the red downward **triangle** that is his health.
//!
//! The game draws its HUD with Scaleform (`interface/flash/widgets/sphud.gfx`); that art is not read here, so the pieces are
//! drawn into small images instead.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::camera::FollowCamera;
use crate::combat::{Dummies, dummy_hp};
use crate::player::Player;
use crate::world::WorldGeometry;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud).add_systems(
            Update,
            (update_cluster, update_enemy_markers, update_prompt).after(crate::orc::update_orcs).after(crate::camera::follow_camera),
        );
    }
}

/// Side of the cluster image (px). It covers 200 px of a 720 px tall frame.
const SIZE: usize = 256;
/// Image pixels per pixel of the 720p reference.
const PX: f32 = SIZE as f32 / 200.0;
/// Radius of the map disc, in reference pixels.
const DISC: f32 = 84.0;
/// World radius the disc shows (m).
const MAP_RANGE: f32 = 32.0;
/// The health arc: from its top end round the left rim to where the screen edge cuts the circle (degrees, counter-clockwise
/// from +x).
const HEALTH_ARC: (f32, f32) = (125.0, 228.0);
const FOCUS_ARC: (f32, f32) = (298.0, 356.0);
/// Player health the arc is full at.
const FULL_HEALTH: f32 = 100.0;

const MARKER: (usize, usize) = (48, 40);

#[derive(Component)]
struct Cluster(Handle<Image>);

/// The traversal prompt bottom right: what the action does and its key in a box ("Drop To Hang [E]").
#[derive(Component)]
struct ActionPrompt;

#[derive(Component)]
struct PromptLabel;

#[derive(Component)]
struct PromptKey;

#[derive(Component)]
struct EnemyMarker {
    dummy: usize,
    image: Handle<Image>,
    /// Health fraction the image was last drawn for.
    drawn: f32,
    /// Seconds since the orc died (the marker fades out).
    gone: f32,
}

fn blank(width: usize, height: usize) -> Image {
    Image::new(
        Extent3d { width: width as u32, height: height as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        vec![0; width * height * 4],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

fn spawn_hud(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let cluster = images.add(blank(SIZE, SIZE));
    // Centre of the disc at (127, 660) of 720: the circle runs off the bottom of the screen, as in the game.
    commands.spawn((
        ImageNode::new(cluster.clone()),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Vh(27.0 / 7.2),
            top: Val::Vh(560.0 / 7.2),
            width: Val::Vh(200.0 / 7.2),
            height: Val::Vh(200.0 / 7.2),
            ..default()
        },
        Cluster(cluster),
    ));
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Vh(3.0),
                bottom: Val::Vh(2.2),
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                ..default()
            },
            Visibility::Hidden,
            ActionPrompt,
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(""),
                TextFont { font_size: bevy::text::FontSize::Px(16.0), ..default() },
                TextColor(Color::srgb(0.92, 0.92, 0.92)),
                PromptLabel,
            ));
            row.spawn((
                Node { padding: UiRect::axes(Val::Px(6.0), Val::Px(1.0)), border: UiRect::all(Val::Px(1.0)), ..default() },
                BackgroundColor(Color::srgba(0.02, 0.03, 0.05, 0.7)),
                BorderColor::all(Color::srgb(0.85, 0.87, 0.9)),
            ))
            .with_children(|b| {
                b.spawn((
                    Text::new(""),
                    TextFont { font_size: bevy::text::FontSize::Px(14.0), ..default() },
                    TextColor(Color::srgb(0.95, 0.97, 1.0)),
                    PromptKey,
                ));
            });
        });
    // Arrow count: a teal arrowhead and the number.
    let mut arrow = blank(32, 32);
    if let Some(data) = arrow.data.as_mut() {
        for y in 0..32 {
            for x in 0..32 {
                let (fx, fy) = (x as f32 - 15.5, y as f32 - 6.0);
                // A downward head with a notch at the top.
                let inside = fy >= 0.0 && fx.abs() < (20.0 - fy) * 0.62 && fy > 7.0 - fx.abs() * 0.9;
                if inside {
                    let shade = if fx < 0.0 { 1.0 } else { 0.72 };
                    data[(y * 32 + x) * 4..][..4].copy_from_slice(&[(70.0 * shade) as u8, (205.0 * shade) as u8, (225.0 * shade) as u8, 255]);
                }
            }
        }
    }
    commands.spawn((
        ImageNode::new(images.add(arrow)),
        Node { position_type: PositionType::Absolute, left: Val::Vh(30.0 / 7.2), top: Val::Vh(524.0 / 7.2), width: Val::Vh(2.6), height: Val::Vh(2.6), ..default() },
    ));
    commands.spawn((
        Text::new("0"),
        TextFont { font_size: bevy::text::FontSize::Px(20.0), ..default() },
        TextColor(Color::srgb(0.92, 0.92, 0.92)),
        Node { position_type: PositionType::Absolute, left: Val::Vh(64.0 / 7.2), top: Val::Vh(521.0 / 7.2), ..default() },
    ));
}

fn update_prompt(
    player: Option<Res<Player>>,
    mut root: Query<&mut Visibility, With<ActionPrompt>>,
    mut label: Query<&mut Text, (With<PromptLabel>, Without<PromptKey>)>,
    mut key: Query<&mut Text, (With<PromptKey>, Without<PromptLabel>)>,
) {
    let (Ok(mut visibility), Ok(mut label), Ok(mut key)) = (root.single_mut(), label.single_mut(), key.single_mut()) else { return };
    match player.and_then(|p| p.hint()) {
        Some((what, keys)) => {
            if label.0 != what {
                label.0 = what.to_string();
                key.0 = keys.to_string();
            }
            *visibility = Visibility::Inherited;
        }
        None => *visibility = Visibility::Hidden,
    }
}

/// Coverage of a band `lo..hi` at `v`, one pixel soft.
fn band(v: f32, lo: f32, hi: f32, soft: f32) -> f32 {
    ((v - lo) / soft + 0.5).clamp(0.0, 1.0) * ((hi - v) / soft + 0.5).clamp(0.0, 1.0)
}

fn over(dst: &mut [f32; 4], src: [f32; 3], alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    for k in 0..3 {
        dst[k] = dst[k] * (1.0 - a) + src[k] * a;
    }
    dst[3] = dst[3] + a * (1.0 - dst[3]);
}

#[allow(clippy::too_many_arguments)]
fn update_cluster(
    time: Res<Time<Real>>,
    player: Option<Res<Player>>,
    dummies: Option<Res<Dummies>>,
    geometry: Option<Res<WorldGeometry>>,
    cameras: Query<&FollowCamera>,
    clusters: Query<&Cluster>,
    mut images: ResMut<Assets<Image>>,
    mut seen: Local<(f32, f32)>,
) {
    let (Some(player), Ok(cluster)) = (player, clusters.single()) else { return };
    let Some(mut image) = images.get_mut(&cluster.0) else { return };
    let yaw = cameras.iter().next().map_or(0.0, |c| c.yaw);
    // The map turns with the camera: its forward is up.
    let forward = Vec2::new(-yaw.sin(), -yaw.cos());
    let right = Vec2::new(yaw.cos(), -yaw.sin());
    let to_map = |world: Vec3| {
        let rel = Vec2::new(world.x - player.pos.x, world.z - player.pos.z);
        Vec2::new(rel.dot(right), rel.dot(forward)) * (DISC / MAP_RANGE)
    };

    // A blow flashes the arc white; low health pulses.
    let (last, flash) = &mut *seen;
    if player.hp < *last - 0.01 {
        *flash = 0.3;
    }
    *last = player.hp;
    *flash = (*flash - time.delta_secs()).max(0.0);
    let health = (player.hp / FULL_HEALTH).clamp(0.0, 1.0);
    let pulse = if health < 0.3 { 0.65 + 0.35 * (time.elapsed_secs() * 7.0).sin() } else { 1.0 };
    let white = (*flash / 0.3).clamp(0.0, 1.0);
    let red = [0.92 * pulse + white * 0.08, 0.05 + white * 0.9, 0.04 + white * 0.9];
    let health_from = HEALTH_ARC.0 + (1.0 - health) * (HEALTH_ARC.1 - HEALTH_ARC.0);

    let orcs: Vec<Vec2> = dummies
        .iter()
        .flat_map(|d| d.0.iter())
        .filter(|d| d.hp > 0.0)
        .map(|d| {
            let m = to_map(d.pos);
            if m.length() > DISC - 5.0 { m.normalize() * (DISC - 5.0) } else { m }
        })
        .collect();
    let pieces: Vec<(Vec3, Vec3)> = geometry.map(|g| g.pieces.clone()).unwrap_or_default();
    // Talion's heading on the map.
    let facing = Vec2::new(-player.yaw.sin(), -player.yaw.cos());
    let heading = Vec2::new(facing.dot(right), facing.dot(forward));

    let soft = 1.0 / PX;
    let mut data = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            // Reference pixels from the disc centre, y up.
            let p = Vec2::new(x as f32 + 0.5 - SIZE as f32 / 2.0, SIZE as f32 / 2.0 - y as f32 - 0.5) / PX;
            let d = p.length();
            if d > 100.0 {
                continue;
            }
            let angle = p.y.atan2(p.x).to_degrees().rem_euclid(360.0);
            let mut c = [0.0f32; 4];
            if d < DISC + 1.0 {
                over(&mut c, [0.13, 0.16, 0.19], 0.82 * band(d, -1.0, DISC, soft));
                // Buildings.
                let world = right * (p.x * MAP_RANGE / DISC) + forward * (p.y * MAP_RANGE / DISC);
                let (wx, wz) = (player.pos.x + world.x, player.pos.z + world.y);
                if pieces.iter().any(|(lo, hi)| wx > lo.x && wx < hi.x && wz > lo.z && wz < hi.z) {
                    over(&mut c, [0.42, 0.47, 0.52], 0.9);
                }
                for o in &orcs {
                    over(&mut c, [0.9, 0.1, 0.08], band((p - *o).length(), -1.0, 3.2, soft));
                }
                // Talion: an arrowhead along his heading.
                let along = p.dot(heading);
                let across = p.dot(Vec2::new(heading.y, -heading.x)).abs();
                if (-5.0..7.0).contains(&along) && across < (7.0 - along) * 0.5 && along > -5.0 + (3.0 - across).max(0.0) * 0.6 {
                    over(&mut c, [0.75, 0.95, 1.0], 1.0);
                }
            }
            // The rim: silver, brighter on its upper right as in the game.
            let bright = if angle < 100.0 || angle > 340.0 { 0.86 } else { 0.42 };
            over(&mut c, [bright, bright, bright * 1.02], band(d, DISC, DISC + 5.0, soft));
            // Health: a dark track with the red arc on it.
            let ring = band(d, DISC + 7.0, DISC + 15.0, soft);
            over(&mut c, [0.16, 0.02, 0.02], 0.75 * ring * band(angle, HEALTH_ARC.0, HEALTH_ARC.1, 0.6));
            over(&mut c, red, ring * band(angle, health_from, HEALTH_ARC.1, 0.6));
            // Focus: teal segments.
            let segment = ((angle - FOCUS_ARC.0) / 9.7).rem_euclid(1.0);
            over(&mut c, [0.1, 0.78, 0.8], ring * band(angle, FOCUS_ARC.0, FOCUS_ARC.1, 0.6) * band(segment, 0.12, 0.88, 0.08));
            let at = (y * SIZE + x) * 4;
            data[at] = (c[0] * 255.0) as u8;
            data[at + 1] = (c[1] * 255.0) as u8;
            data[at + 2] = (c[2] * 255.0) as u8;
            data[at + 3] = (c[3] * 255.0) as u8;
        }
    }
    image.data = Some(data);
}

/// The enemy's health marker: a downward triangle, red as far as his health goes, hollow above the rest.
fn draw_marker(fraction: f32) -> Vec<u8> {
    let (w, h) = MARKER;
    let mut data = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let fx = (x as f32 + 0.5 - w as f32 / 2.0).abs();
            let fy = y as f32 + 0.5;
            // Half width shrinks to the tip at the bottom.
            let edge = (h as f32 - 2.0 - fy) * (w as f32 / 2.0 - 2.0) / (h as f32 - 4.0);
            let inside = band(fx, -1.0, edge, 1.0) * band(fy, 2.0, h as f32, 1.0);
            if inside <= 0.0 {
                continue;
            }
            let border = fx > edge - 3.5 || fy < 5.5;
            // Health empties from the tip upward.
            let filled = fy <= 2.0 + (h as f32 - 4.0) * fraction;
            let mut c = [0.0f32; 4];
            if filled {
                over(&mut c, [0.93, 0.07, 0.05], inside);
            } else {
                over(&mut c, [0.1, 0.0, 0.0], inside * 0.45);
            }
            if border {
                over(&mut c, [0.45, 0.02, 0.02], inside * 0.9);
            }
            let at = (y * w + x) * 4;
            data[at] = (c[0] * 255.0) as u8;
            data[at + 1] = (c[1] * 255.0) as u8;
            data[at + 2] = (c[2] * 255.0) as u8;
            data[at + 3] = (c[3] * 255.0) as u8;
        }
    }
    data
}

fn update_enemy_markers(
    mut commands: Commands,
    time: Res<Time<Real>>,
    dummies: Option<Res<Dummies>>,
    player: Option<Res<Player>>,
    cameras: Query<(&Camera, &GlobalTransform), With<FollowCamera>>,
    windows: Query<&Window>,
    mut markers: Query<(&mut EnemyMarker, &mut Node, &mut Visibility, &mut ImageNode)>,
    mut images: ResMut<Assets<Image>>,
) {
    let (Some(dummies), Ok((camera, camera_transform)), Ok(window)) = (dummies, cameras.single(), windows.single()) else { return };
    // One marker per orc, made when the orcs exist.
    for dummy in markers.iter().count()..dummies.0.len() {
        let image = images.add(blank(MARKER.0, MARKER.1));
        commands.spawn((
            ImageNode::new(image.clone()),
            Node { position_type: PositionType::Absolute, width: Val::Vh(2.6), height: Val::Vh(2.6 * MARKER.1 as f32 / MARKER.0 as f32), ..default() },
            Visibility::Hidden,
            EnemyMarker { dummy, image, drawn: -1.0, gone: 0.0 },
        ));
    }
    let player_pos = player.map_or(Vec3::ZERO, |p| p.pos);
    let size = window.height() * 0.026;
    for (mut marker, mut node, mut visibility, mut image_node) in &mut markers {
        let Some(dummy) = dummies.0.get(marker.dummy) else { continue };
        let fraction = (dummy.hp / dummy_hp()).clamp(0.0, 1.0);
        if (fraction - marker.drawn).abs() > 0.004 {
            marker.drawn = fraction;
            if let Some(mut image) = images.get_mut(&marker.image) {
                image.data = Some(draw_marker(fraction));
            }
        }
        marker.gone = if dummy.hp > 0.0 { 0.0 } else { marker.gone + time.delta_secs() };
        // He is in the fight once he is close or hurt; the marker lingers a moment on a kill.
        let near = Vec2::new(dummy.pos.x - player_pos.x, dummy.pos.z - player_pos.z).length() < 12.0;
        let shown = if dummy.hp > 0.0 { near || fraction < 1.0 } else { marker.gone < 0.6 };
        match camera.world_to_viewport(camera_transform, dummy.pos + Vec3::Y * 1.8) {
            Ok(screen) if shown => {
                node.left = Val::Px(screen.x - size / 2.0);
                node.top = Val::Px(screen.y - size);
                image_node.color = Color::srgba(1.0, 1.0, 1.0, (1.0 - marker.gone / 0.6).clamp(0.0, 1.0));
                *visibility = Visibility::Inherited;
            }
            _ => *visibility = Visibility::Hidden,
        }
    }
}
