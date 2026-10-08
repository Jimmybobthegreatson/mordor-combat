//! The arena: a flat, solid baseplate. (The full project also places the game's towers and ruins; this build has no world geometry.)

use bevy::prelude::*;

/// Body radius used against walls (m).
pub const BODY_RADIUS: f32 = 0.4;

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldGeometry>();
    }
}

/// Collision queries over the arena. There is only the floor at height 0.
#[derive(Resource, Default)]
pub struct WorldGeometry {
    /// Bounds of placed pieces, for the minimap (none on the bare baseplate).
    pub pieces: Vec<(Vec3, Vec3)>,
}

impl WorldGeometry {
    pub fn raycast(&self, _origin: Vec3, _dir: Vec3, _max: f32) -> Option<(f32, Vec3)> {
        None
    }

    /// Height of the floor under `pos`.
    pub fn ground(&self, _pos: Vec3, _step: f32) -> f32 {
        0.0
    }

    pub fn push_out(&self, pos: Vec3, _radius: f32) -> Vec3 {
        pos
    }
}
