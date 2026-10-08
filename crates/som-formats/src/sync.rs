//! Placement of a victim relative to the player for a synchronised animation pair.
//!
//! The game's sync records carry no offsets (`Attach` mode): both clips are authored with their root at the
//! origin and the engine lines the characters up. Here the alignment is recovered from the clips: the
//! victim's start pose (a yaw and an offset in the player's start frame) is chosen so that, around the
//! moment of impact, the player's weapon hand and the victim's torso coincide as closely as possible.

use crate::animator::{ClipId, Library};

/// Where the victim starts, in the player's clip-start frame (game space: x right, z forward, cm).
#[derive(Debug, Clone, Copy)]
pub struct SyncFit {
    /// Victim root position (cm) in the player's frame.
    pub offset: [f32; 2],
    /// Victim heading (radians, game yaw about +Y) in the player's frame.
    pub yaw: f32,
    /// Mean distance (cm) between the hand and the victim's torso over the fitted window.
    pub residual: f32,
}

/// Like [`fit`], but searching only near the usual arrangement: the victim `distance` cm away on the player's
/// `front` (z) or back side, facing him, within +-`slack` cm and +-40 degrees. Constrains the fit to a sane start.
#[allow(clippy::too_many_arguments)]
pub fn fit_near(
    player: &Library,
    player_clip: ClipId,
    hand: usize,
    victim: &Library,
    victim_clip: ClipId,
    torso: usize,
    blade_cm: f32,
    window: (f32, f32),
    front: bool,
    distance: f32,
    slack: f32,
) -> Option<SyncFit> {
    let p0 = player.start_ms(player_clip);
    let v0 = victim.start_ms(victim_clip);
    let (mut hands, mut torsos) = (Vec::new(), Vec::new());
    for k in 0..=12 {
        let t = (window.0 + (window.1 - window.0) * k as f32 / 12.0) * 1000.0;
        let ps = player.sample(player_clip, p0 + t).ok()?;
        let vs = victim.sample(victim_clip, v0 + t).ok()?;
        let (hp, hr) = player.bone_transform(&ps, hand);
        let h = crate::math::vadd(hp, crate::math::qrot(hr, [0.0, blade_cm, 0.0]));
        let v = victim.bone_position(&vs, torso);
        hands.push([h[0], h[2]]);
        torsos.push([v[0], v[2]]);
    }
    let (base_z, base_yaw) = if front { (distance, std::f32::consts::PI) } else { (-distance, 0.0) };
    let mut best: Option<SyncFit> = None;
    for dyaw in (-40..=40).step_by(4) {
        let yaw = base_yaw + (dyaw as f32).to_radians();
        let (s, c) = yaw.sin_cos();
        for ix in (-(slack as i32)..=slack as i32).step_by(6) {
            for iz in (-(slack as i32)..=slack as i32).step_by(6) {
                let offset = [ix as f32, base_z + iz as f32];
                let residual = hands
                    .iter()
                    .zip(&torsos)
                    .map(|(h, p)| {
                        let r = [p[0] * c + p[1] * s + offset[0], -p[0] * s + p[1] * c + offset[1]];
                        ((h[0] - r[0]).powi(2) + (h[1] - r[1]).powi(2)).sqrt()
                    })
                    .sum::<f32>()
                    / hands.len() as f32;
                if best.is_none_or(|b| residual < b.residual) {
                    best = Some(SyncFit { offset, yaw, residual });
                }
            }
        }
    }
    best
}

/// Fit the victim's placement. `window` is the time range (seconds into the clips) to match over, normally a
/// little before to just after the blow. `hand` and `torso` are bone indices in the player's and victim's skeletons;
/// the point matched is `blade_cm` along the weapon bone's +Y (the blade) from the hand.
pub fn fit(
    player: &Library,
    player_clip: ClipId,
    hand: usize,
    victim: &Library,
    victim_clip: ClipId,
    torso: usize,
    blade_cm: f32,
    window: (f32, f32),
) -> Option<SyncFit> {
    let p0 = player.start_ms(player_clip);
    let v0 = victim.start_ms(victim_clip);
    let mut hands = Vec::new();
    let mut torsos = Vec::new();
    let steps = 12;
    for k in 0..=steps {
        let t = (window.0 + (window.1 - window.0) * k as f32 / steps as f32) * 1000.0;
        let ps = player.sample(player_clip, p0 + t).ok()?;
        let vs = victim.sample(victim_clip, v0 + t).ok()?;
        let (hp, hr) = player.bone_transform(&ps, hand);
        let h = crate::math::vadd(hp, crate::math::qrot(hr, [0.0, blade_cm, 0.0]));
        let v = victim.bone_position(&vs, torso);
        hands.push([h[0], h[2]]);
        torsos.push([v[0], v[2]]);
    }
    let mut best: Option<SyncFit> = None;
    for deg in (0..360).step_by(3) {
        let yaw = (deg as f32).to_radians();
        let (s, c) = yaw.sin_cos();
        // A game yaw turns +Z toward +X: (x, z) -> (x cos + z sin, -x sin + z cos).
        let rotated: Vec<[f32; 2]> = torsos.iter().map(|p| [p[0] * c + p[1] * s, -p[0] * s + p[1] * c]).collect();
        let n = hands.len() as f32;
        let offset = [
            hands.iter().zip(&rotated).map(|(h, r)| h[0] - r[0]).sum::<f32>() / n,
            hands.iter().zip(&rotated).map(|(h, r)| h[1] - r[1]).sum::<f32>() / n,
        ];
        let residual = hands
            .iter()
            .zip(&rotated)
            .map(|(h, r)| ((h[0] - r[0] - offset[0]).powi(2) + (h[1] - r[1] - offset[1]).powi(2)).sqrt())
            .sum::<f32>()
            / n;
        if best.is_none_or(|b| residual < b.residual) {
            best = Some(SyncFit { offset, yaw, residual });
        }
    }
    best
}
