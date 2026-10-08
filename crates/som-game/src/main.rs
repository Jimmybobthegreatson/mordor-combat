//! Shadow of Mordor combat playground: a stick figure against an orc on a flat baseplate, animated by the game's own data.
//! The game's install folder must be chosen before it runs (see [`install`]).

mod camera;
mod gait;
mod hud;
mod install;
mod combat;
mod character;
mod orc;
mod player;
mod shot;
mod world;

use bevy::prelude::*;

fn main() {
    install::ensure();
    let options = shot::Options::from_args();
    let mut app = App::new();
    app.insert_resource(options)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "mordor-combat".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((character::CharacterPlugin, shot::ShotPlugin))
        .add_systems(Startup, setup_baseplate)
        .add_systems(Update, draw_grid);
    app.add_plugins((camera::FollowCameraPlugin, player::PlayerPlugin, combat::DummyPlugin, orc::OrcPlugin, world::WorldPlugin, hud::HudPlugin));
    app.run();
}

/// Half-extent of the baseplate in metres.
const PLATE_HALF: f32 = 50.0;

fn setup_baseplate(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(PLATE_HALF * 2.0, PLATE_HALF * 2.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.32, 0.33, 0.35),
            perceptual_roughness: 0.95,
            ..default()
        })),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(-7.0, 12.0, -9.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // Soft fill from the other side so the back of the model is not black.
    commands.spawn((
        DirectionalLight { illuminance: 3_500.0, ..default() },
        Transform::from_xyz(8.0, 5.0, 7.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        brightness: 150.0,
        ..default()
    });
}

/// Metre grid on the plate (gizmos, so the baseplate stays a single mesh).
fn draw_grid(mut gizmos: Gizmos) {
    let n = PLATE_HALF as i32;
    for i in -n..=n {
        let major = i % 10 == 0;
        let color = if major { Color::srgba(1.0, 1.0, 1.0, 0.35) } else { Color::srgba(1.0, 1.0, 1.0, 0.08) };
        let f = i as f32;
        gizmos.line(Vec3::new(f, 0.01, -PLATE_HALF), Vec3::new(f, 0.01, PLATE_HALF), color);
        gizmos.line(Vec3::new(-PLATE_HALF, 0.01, f), Vec3::new(PLATE_HALF, 0.01, f), color);
    }
}
