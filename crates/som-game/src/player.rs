//! The playable character: camera-relative input, a locomotion + combat state machine and
//! root-motion driven movement over the baseplate.
//!
//! Clips are the game's own: idle, walk and run loops, start / stop / turning transitions, the
//! freeflow attack chain, counters, hurt reactions and the evade roll. The animator strips the `Null`
//! bone's travel and yaw out of the pose and the entity takes them over, so feet stay planted and
//! turning clips really turn the character as authored. Attack timing (the blow, the hit window,
//! when the next attack may start) comes from the cues baked into each clip.

use bevy::prelude::*;
use som_formats::animator::{Animator, ClipId, Library};

use crate::camera::FollowCamera;
use crate::character::{Rig, to_bevy_quat, to_bevy_vec};
use crate::combat::Select as _;
use crate::combat::{Catalog, Dummies, STAND_OFF};
use crate::shot::Options;

/// Banks the locomotion clips come from, in the order [`Clips`] indexes them.
pub const BANKS: [&str; 4] = [
    "animation/player/coremovement/pl_movbase.anix",
    "animation/player/coremovement/pl_movaction.anix",
    "animation/player/coremovement/pl_transbase.anix",
    "animation/player/coremovement/pl_movtrans.anix",
];

/// Half-extent of the baseplate (m); the player stays inside it.
const PLATE_LIMIT: f32 = 49.0;

/// Turn rates in radians per second: standing starts are quick, moving turns wide.
const TURN_START: f32 = 9.0;
const TURN_MOVING: f32 = 6.0;
/// Gentle correction while a turning clip is already rotating the character.
const TURN_CORRECT: f32 = 1.2;
/// Seconds out of combat before the sword goes back on his back.
/// Seconds after the last combat during which the gait trees take their combat branch (`InCombat`).
const COMBAT_MEMORY: f32 = 8.0;
/// Seconds without landing a hit before the hit counter resets.
/// What Talion carries for the move predicates: the gore setting and the Execution ability.
const ITEMS: &[&str] = &["CSP_HeadExp", "CSP_Chord_InstaK"];
/// Consecutive hits that charge the Hit Streak ("every x8 hits"): a charge buys one Execution.
const STREAK: u32 = 8;
/// Seconds the swing carries on after a landed blow before a buffered attack cuts in.
const FOLLOW_THROUGH: f32 = 0.12;
const COMBO_TIMEOUT: f32 = 4.0;
/// Damage of a blow by hit tier (0-3, 4-15, 16-29, 30+ hits); heavy strikes add to it.
const DAMAGE: [f32; 5] = [0.0, 14.0, 18.0, 24.0, 34.0];

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (init_player, prune_sync_moves, drive_player, update_sword, update_combo_hud).chain().before(crate::camera::follow_camera),
        )
        .add_systems(Startup, spawn_hud);
    }
}

/// Left/right variants of a turning clip.
#[derive(Clone, Copy)]
struct Pair {
    left: ClipId,
    right: ClipId,
}

impl Pair {
    /// The variant that turns toward `angle` (positive = left).
    fn pick(self, angle: f32) -> ClipId {
        if angle > 0.0 { self.left } else { self.right }
    }
}

#[derive(Clone, Copy)]
struct Clips {
    idle: ClipId,
    walk: ClipId,
    run: ClipId,
    start_run: ClipId,
    start_walk: ClipId,
    stop_run: ClipId,
    stop_walk: ClipId,
    /// Standing turn-and-go starts, by turn size in degrees (45, 90, 135, 180).
    run_starts: [Pair; 4],
    /// Walking starts for 90 and 180 degrees.
    walk_starts: [Pair; 2],
    run_pivot_90: Pair,
    run_turn_180: Pair,
    walk_turn_out: Pair,
    /// Walking about-face that ends standing, one variant per leading foot.
    walk_turn_180: [Pair; 2],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Loco {
    Idle,
    /// Leaving standstill; `turning` when the clip itself swings the character round.
    Starting { run: bool, turning: bool },
    Moving { run: bool },
    /// Pivot or about-face while moving; `then_stop` for the walking 180 that ends standing.
    Turning { run: bool, then_stop: bool },
    Stopping,
    /// Hit tier 1..=4 of the attack, or 0 for a get-there gap closer.
    Attack { level: u8 },
    /// A synchronised counter with an attacking orc.
    Counter,
    Hurt,
    Roll,
}

impl Player {
    /// The traversal action on offer (what it does, its key), for the prompt bottom right.
    pub fn hint(&self) -> Option<(&'static str, &'static str)> {
        None
    }

    pub fn is_crouching(&self) -> bool {
        self.crouching
    }

    /// The Hit Streak is charged: an Execution is available.
    pub fn is_charged(&self) -> bool {
        self.combat.charged
    }

    /// Drop the synchronised moves whose victim half the orcs do not have (the audit lists them), so they are never picked.
    pub fn prune_moves(&mut self, has: impl Fn(&str) -> bool) {
        let moves = &mut self.catalog.moves;
        let missing: Vec<usize> = moves
            .attacks
            .iter()
            .enumerate()
            .filter(|(_, a)| a.victim_clip.as_ref().is_some_and(|v| !has(v) && !has(&v.replace("_NoSev", ""))))
            .map(|(i, _)| i)
            .collect();
        for node in &mut moves.nodes {
            node.attacks.retain(|i| !missing.contains(i));
        }
        info!("pruned {} synchronised moves without a victim clip", missing.len());
    }

    /// The hit counter.
    pub fn combo(&self) -> u32 {
        self.combat.combo
    }
}

impl Player {
    /// The camera numbers for what he is doing now (`CameraBlendState` of the player graph).
    pub fn camera_profile(&self) -> Option<som_formats::bvrgraph::CameraProfile> {
        self.gaits.as_ref()?.camera("Camera_OnGround")
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }
}

#[derive(Resource)]
pub struct Player {
    pub pos: Vec3,
    pub hp: f32,
    weapon_bone: Option<usize>,
    /// Heading about Y (radians); 0 faces -Z, the way the model faces in Bevy. Positive turns left.
    pub yaw: f32,
    state: Loco,
    animator: Animator,
    clips: Clips,
    /// The clip sets for standing and for crouching (stealth).
    standing: Clips,
    crouched: Clips,
    crouching: bool,
    crouch_held: bool,
    /// Speed over the last frame (m/s), for the HUD.
    speed: f32,
    attack_held: bool,
    finisher_held: bool,
    roll_held: bool,
    hurt_held: bool,
    counter_held: bool,
    catalog: Catalog,
    combat: Combat,
    /// The player behaviour graph: gait clips and rates, camera profiles.
    gaits: Option<std::sync::Arc<crate::gait::GaitGraph>>,
    /// Playback rate of the graph's gait state, and seconds in it (`Game_NodeLifeTime`).
    gait_rate: f32,
    gait_time: f32,
}

/// Where the combat chain is.
#[derive(Default)]
struct Combat {
    /// Foot stance the next attack should start from (`LL`, `LR`, `RL`, `RR`).
    stance: String,
    /// Another attack was pressed during this one.
    queued: bool,
    hit_done: bool,
    /// When the blow lands, from the clip's `APPLYDAMAGE` cue (seconds into the clip).
    hit_s: f32,
    /// The next attack comes from F: the finisher (flurry / ender nodes) on a stunned orc.
    finisher_next: bool,
    /// The Hit Streak is charged (every `STREAK` consecutive hits); F spends it on an Execution.
    charged: bool,
    /// Sprinting into an attack: the dash attack nodes.
    dash_next: bool,
    /// Which counter family comes next.
    counter_cycle: usize,
    /// What the attack in progress does to its victim (stun / knock-down states from the data), and whether it is a leg sweep.
    applied: Vec<som_formats::combatnodes::Applied>,
    leg: bool,
    /// The outcome of the sync action in progress: lethal, or the states it applies (stun / knock-down / knock-back counters).
    sync_lethal: bool,
    /// The sync action in progress is no blow at all (the vault): nothing is applied and the hit counter does not move.
    sync_harmless: bool,
    sync_applied: Vec<som_formats::combatnodes::Applied>,
    /// Distance from the blade to the nearest target last frame (blow detection: the blade has passed its closest point).
    blade_gap: f32,
    /// When (seconds into the clip) the blow of the attack in progress landed on an orc.
    landed_at: Option<f32>,
    /// The slow motion this attack earns (kill blow, or the hit that charges the streak), and whether it has started.
    /// The orc the attack in progress is aimed at.
    target: Option<usize>,
    /// The attack in progress keeps facing its target.
    steer: bool,
    blade_min: (f32, f32),
    /// When the next attack may begin (`BUFFTRANS`).
    chain_s: f32,
    /// Scale on the clip's root travel so it lands on the target.
    warp: f32,
    /// Turn still to apply to line the clip up with its target (radians, positive = left).
    pending_turn: f32,
    variety: usize,
    combo: u32,
    last_attack: String,
    /// The last few attack clips, so the chain does not repeat itself.
    recent: Vec<String>,
    /// The current blow is a heavy strike.
    heavy: bool,
    swing: &'static str,
    /// Weapon collision window (`BCOL`..`ECOL`) of the current blow, seconds into the clip.
    collide: (f32, f32),
    /// The sync action in progress (counter, finisher, stealth kill): orc index and whether the kill has landed.
    counter: Option<(usize, bool)>,
    /// When (seconds into the clip) the sync action's blow lands.
    sync_kill: f32,
    /// The dagger is out (stealth kills use it, not the sword).
    dagger: bool,
    /// Flurry hits landed on the current stunned target; the ender follows.
    flurry: u32,
    /// From this time into the clip (`BCANWIN`) he may cancel into a roll or counter.
    cancel_s: f32,
    /// Window (`STARTFACING`..`STOPFACING`) in which the clip turns him toward the target.
    face: Option<(f32, f32)>,
    /// Seconds since the last landed hit; the combo ends after a few.
    since_hit: f32,
    hurt_cycle: usize,
    /// The sword is in his hand.
    drawn: bool,
    /// Seconds since combat last happened.
    calm: f32,
}

/// The clip and rate of the graph's gait state for the keys held: `Sprint` for running, `Walking` otherwise (`Walking` / `Sprint` and their
/// clip trees from `player_elf.bvr`; crouching and the sword being out pick the tree's stealth / combat branches).
fn gait(player: &Player, lib: &Library, run: bool) -> Option<(ClipId, f32)> {
    let ctx = crate::gait::GroundCtx { stealth: player.crouching, in_combat: player.combat.calm < COMBAT_MEMORY, node_time: player.gait_time, move_mag: 1.0 };
    let state = if run { "Sprint" } else { "Walking" };
    let picked = player.gaits.as_ref().and_then(|g| g.steady(state, ctx, lib));
    if std::env::var_os("SOM_GAIT_DEBUG").is_some() {
        if let Some((clip, rate)) = picked {
            eprintln!("[gait] {state} t={:.2} -> {} rate {rate:.2}", player.gait_time, lib.name(clip));
        }
    }
    picked
}

fn init_player(mut commands: Commands, rig: Option<Res<Rig>>, player: Option<Res<Player>>, options: Res<Options>) {
    let (Some(rig), None) = (rig, player) else { return };
    let find = |bank, name: &str| rig.lib.clip(bank, name).ok_or_else(|| anyhow::anyhow!("clip {name} not found"));
    let pair = |bank, left: &str, right: &str| -> anyhow::Result<Pair> {
        Ok(Pair { left: find(bank, left)?, right: find(bank, right)? })
    };
    let clips = (|| -> anyhow::Result<Clips> {
        Ok(Clips {
            idle: find(0, "PL_Base_L_Fist_IdleBase")?,
            walk: find(0, "PL_Cmbt_Walk")?,
            run: find(1, "PL_Base_L_Fist_RunAction")?,
            start_run: find(3, "PL_Idle_Idle2Run")?,
            start_walk: find(3, "PL_Idle_Idle2Walk")?,
            stop_run: find(2, "PL_Base_L_Fist_RunToIdle")?,
            stop_walk: find(2, "Bat_Base_R_Fist_WalkToIdle")?,
            run_starts: [
                pair(3, "PL_Idle_Idle2Run_45L", "PL_Idle_Idle2Run_45R")?,
                pair(3, "PL_Idle_Idle2Run_90L", "PL_Idle_Idle2Run_90R")?,
                pair(3, "PL_Idle_Idle2Run_135L", "PL_Idle_Idle2Run_135R")?,
                pair(3, "PL_Idle_Idle2Run_180L", "PL_Idle_Idle2Run_180R")?,
            ],
            walk_starts: [
                pair(3, "PL_Idle_Idle2Walk_90L", "PL_Idle_Idle2Walk_90R")?,
                pair(3, "PL_Idle_Idle2Walk_180L", "PL_Idle_Idle2Walk_180R")?,
            ],
            run_pivot_90: pair(1, "PL_Base_R_Fist_RunPivot90L", "PL_Base_L_Fist_RunPivot90R")?,
            run_turn_180: pair(1, "PL_Base_R_Fist_Run180L", "PL_Base_L_Fist_Run180R")?,
            walk_turn_out: pair(1, "PL_Walk_TurnOut_L", "PL_Walk_TurnOut_R")?,
            walk_turn_180: [
                pair(1, "PL_Walk_HockeyStop_180L_L", "PL_Walk_HockeyStop_180R_L")?,
                pair(1, "PL_Walk_HockeyStop_180L_R", "PL_Walk_HockeyStop_180R_R")?,
            ],
        })
    })();
    let clips = match clips {
        Ok(clips) => clips,
        Err(err) => {
            error!("locomotion clips unavailable: {err:#}");
            return;
        }
    };
    // Crouching uses the game's stealth set; anything it lacks falls back to the standing clip.
    let opt = |bank, name: &str, fallback: ClipId| rig.lib.clip(bank, name).unwrap_or(fallback);
    let pair_opt = |bank, left: &str, right: &str, fallback: Pair| Pair {
        left: opt(bank, left, fallback.left),
        right: opt(bank, right, fallback.right),
    };
    let crouched = Clips {
        idle: opt(1, "PL_Stlh_L_Idle", clips.idle),
        walk: opt(1, "PL_Walk_Stlh", clips.walk),
        run: opt(1, "PL_Run_Stlh", clips.run),
        start_run: opt(1, "PL_RunStart_Stlh", clips.start_run),
        start_walk: opt(1, "PL_WalkStart_Stlh", clips.start_walk),
        stop_run: opt(1, "PL_RunStop_Stlh", clips.stop_run),
        stop_walk: opt(1, "PL_WalkStop_Stlh", clips.stop_walk),
        run_starts: [
            pair_opt(3, "Stlh_45L", "Stlh_45R", clips.run_starts[0]),
            pair_opt(3, "Stlh_90L", "Stlh_90R", clips.run_starts[1]),
            pair_opt(3, "Stlh_135L", "Stlh_135R", clips.run_starts[2]),
            pair_opt(3, "Stlh_180L", "Stlh_180R", clips.run_starts[3]),
        ],
        walk_starts: [
            pair_opt(3, "Stlh_90L", "Stlh_90R", clips.walk_starts[0]),
            pair_opt(3, "Stlh_180L", "Stlh_180R", clips.walk_starts[1]),
        ],
        ..clips
    };
    let mut animator = Animator::new();
    if let Err(err) = animator.play(&rig.lib, clips.idle, true, 0.0, 0.0) {
        error!("cannot start idle: {err:#}");
    }
    let mut player = Player {
        pos: options.start.map_or(Vec3::ZERO, |(x, z, _)| Vec3::new(x, std::env::var("SOM_START_Y").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0), z)),
        hp: 100.0,
        weapon_bone: rig.lib.find_bone("R_Weapon"),
        yaw: options.start.map_or(0.0, |(_, _, yaw)| yaw.to_radians()),
        state: Loco::Idle,
        animator,
        clips,
        standing: clips,
        crouched,
        crouching: false,
        gait_rate: 1.0,
        gait_time: 0.0,
        crouch_held: false,
        speed: 0.0,
        attack_held: false,
        finisher_held: false,
        roll_held: false,
        hurt_held: false,
        counter_held: false,
        catalog: Catalog::build(&rig.lib, &rig.attacks, &rig.moves),
        combat: Combat { stance: "LL".into(), warp: 1.0, drawn: true, calm: 99.0, ..default() },
        gaits: crate::gait::GaitGraph::load().map_err(|err| error!("no gait graph: {err:#}")).ok(),
    };
    commands.insert_resource(player);
}

/// Wanted move direction on the ground (unit length or zero) and whether to walk.
fn read_input(options: &Options, keys: &ButtonInput<KeyCode>, time: &Time, camera_yaw: f32) -> (Vec3, bool) {
    let down = |k| options.pressed(keys, time, k);
    let x = down(KeyCode::KeyD) as i32 - down(KeyCode::KeyA) as i32;
    let y = down(KeyCode::KeyW) as i32 - down(KeyCode::KeyS) as i32;
    let forward = Vec3::new(-camera_yaw.sin(), 0.0, -camera_yaw.cos());
    let right = Vec3::new(camera_yaw.cos(), 0.0, -camera_yaw.sin());
    let dir = forward * y as f32 + right * x as f32;
    // He walks; holding Shift runs.
    (dir.normalize_or_zero(), !down(KeyCode::ShiftLeft))
}

/// When the playing clip fires the cue `name` (seconds into the clip), or `default`.
fn cue_of(player: &Player, lib: &Library, name: &str, default: f32) -> f32 {
    player.animator.current().and_then(|clip| lib.event_s(clip, name)).unwrap_or(default)
}

fn shortest_angle(from: f32, to: f32) -> f32 {
    (to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// The standing turn-and-go start for a turn of `angle` radians (positive = left), if one is needed.
fn start_turn(clips: &Clips, run: bool, angle: f32) -> Option<ClipId> {
    let deg = angle.abs().to_degrees();
    if run {
        let i = match deg {
            d if d < 25.0 => return None,
            d if d < 67.0 => 0,
            d if d < 112.0 => 1,
            d if d < 157.0 => 2,
            _ => 3,
        };
        Some(clips.run_starts[i].pick(angle))
    } else {
        match deg {
            d if d < 50.0 => None,
            d if d < 135.0 => Some(clips.walk_starts[0].pick(angle)),
            _ => Some(clips.walk_starts[1].pick(angle)),
        }
    }
}

/// Start a synchronised pair on `orc`: Talion plays `player_clip` and the orc `victim`. Returns false if a clip is missing.
#[allow(clippy::too_many_arguments)]
fn begin_sync(
    player: &mut Player,
    lib: &Library,
    counters: &mut crate::orc::CounterState,
    orc: usize,
    front: bool,
    player_clip: &str,
    victim: String,
    place: crate::orc::Place,
    dagger: bool,
) -> bool {
    let Some(clip) = lib.find(player_clip) else {
        warn!("sync clip {player_clip} missing");
        return false;
    };
    let seconds = lib.duration_ms(clip) / 1000.0;
    // The blow lands a little before the clip's hit window closes.
    player.combat.sync_kill = lib.event_s(clip, "BHITWIN").map(|t| (t - 0.3).max(0.2)).unwrap_or(seconds * 0.5);
    player.combat.dagger = dagger;
    player.combat.sync_lethal = true;
    player.combat.sync_harmless = false;
    player.combat.sync_applied.clear();
    if !dagger {
        player.combat.drawn = true;
    }
    player.combat.calm = 0.0;
    player.combat.counter = Some((orc, false));
    player.combat.pending_turn = 0.0;
    player.combat.warp = 1.0;
    player.state = Loco::Counter;
    counters.request = Some(crate::orc::CounterRequest { orc, front, victim, place });
    counters.prompt = None;
    info!("sync {player_clip} on orc {orc}");
    if let Err(err) = player.animator.play(lib, clip, false, 0.05, 0.0) {
        error_once!("animation failed: {err:#}");
    }
    true
}

/// The runtime attack for a node's `AttackDef` (clip, side, swing, travel), or `None` when its clip is not in a loaded bank.
fn attack_from_def(lib: &Library, def: &som_formats::combatnodes::AttackDef, stance: &str) -> Option<crate::combat::Attack> {
    let clip = lib.find(&def.clip)?;
    let word = def.allowed.split('/').find(|w| !w.is_empty()).unwrap_or("Forward");
    // The clip is authored for one side (the `_D1_F_` token of its name); a few records list it for another side, and then the
    // clip's own side is the one to line up with.
    let parts: Vec<&str> = def.clip.split('_').collect();
    let authored = parts
        .iter()
        .position(|p| p.len() == 2 && p.starts_with('D') && p[1..].chars().all(|c| c.is_ascii_digit()))
        .and_then(|d| parts.get(d + 1))
        .and_then(|side| match *side {
            "F" => Some(crate::combat::Dir::Front),
            "B" => Some(crate::combat::Dir::Back),
            "L" => Some(crate::combat::Dir::Left),
            "R" => Some(crate::combat::Dir::Right),
            _ => None,
        });
    let dir = authored
        .or_else(|| crate::combat::DIRS.into_iter().find(|d| crate::combat::side_word(*d) == word))
        .unwrap_or(crate::combat::Dir::Front);
    let swing = if def.name.contains("R2L") || def.kind.contains("RightToLeft") {
        "R2L"
    } else if def.name.contains("L2R") || def.kind.contains("LeftToRight") {
        "L2R"
    } else if def.kind.contains("UpToDown") {
        "U2D"
    } else if def.kind.contains("DownToUp") {
        "D2U"
    } else {
        "ST"
    };
    Some(crate::combat::Attack {
        name: def.name.clone(),
        clip,
        hits: def.hits,
        tier: crate::combat::tier_of(def.hits.0),
        dir,
        allowed: vec![dir],
        start: if def.start.is_empty() { stance.to_string() } else { def.start.clone() },
        end: if def.end.is_empty() { stance.to_string() } else { def.end.clone() },
        heavy: def.kind.starts_with("Heavy"),
        swing,
        range_cm: def.range_cm,
        reach: lib.root_total(clip).map(|(t, _)| t[0].hypot(t[2]) / 100.0).unwrap_or(0.0),
        strike_reach: crate::combat::strike_reach_of(lib, clip),
    })
}

/// Start the next attack of the chain (`first` when it opens one) against the best target, or a gap closer first when the
/// target is far away. Chooses the clip by the target's side and distance and the current foot stance.
fn begin_attack(
    player: &mut Player,
    lib: &Library,
    dummies: &Dummies,
    counters: &mut crate::orc::CounterState,
    first: bool,
    wanted_dir: Vec3,
    fade: f32,
) {
    let facing = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
    let search = if wanted_dir != Vec3::ZERO { wanted_dir } else { facing };
    let target = dummies
        .1
        .filter(|&i| dummies.0[i].hp > 0.0)
        .or_else(|| dummies.nearest(player.pos, search, 8.0, 55f32.to_radians()))
        .or_else(|| dummies.nearest(player.pos, facing, 2.2, std::f32::consts::PI));
    let (side, distance) = match target {
        Some(i) => {
            let to = dummies.0[i].pos - player.pos;
            (shortest_angle(player.yaw, (-to.x).atan2(-to.z)), Some(Vec2::new(to.x, to.z).length()))
        }
        None => (0.0, None),
    };
    player.combat.variety += 1;
    player.combat.drawn = true;
    player.combat.calm = 0.0;

    // Far away: close the gap first, then attack.
    if first && distance.is_some_and(|d| d > 4.8) && side.abs() < 1.0 {
        let need = distance.unwrap_or(0.0) - STAND_OFF - 1.4;
        if let Some(&(clip, reach)) =
            player.catalog.get_there.iter().min_by(|a, b| (a.1 - need).abs().total_cmp(&(b.1 - need).abs()))
        {
            player.combat.warp = (need / reach.max(0.3)).clamp(0.5, 1.6);
            player.combat.pending_turn = side;
            player.combat.hit_done = true;
            player.combat.queued = false;
            player.state = Loco::Attack { level: 0 };
            if let Err(err) = player.animator.play(lib, clip, false, fade, 0.0) {
                error_once!("animation failed: {err:#}");
            }
            return;
        }
    }

    let stance = player.combat.stance.clone();
    let finisher = std::mem::take(&mut player.combat.finisher_next);
    let dash = std::mem::take(&mut player.combat.dash_next);
    // What the game's nodes allow against this target and state: the finisher (F) on a stunned orc, ground strikes on one lying
    // down (`PC_PoolAttacks_Hvy`, `KnockedDown`), the knock-down sweeps on a stunned one (`PC_PoolAttacks_KD`, `StunnedOnly`), the
    // dash attack when sprinting, otherwise the freeflow chain.
    let mut node_attack: Option<crate::combat::Attack> = None;
    if let (Some(i), Some(d)) = (target, distance) {
        let dn = &dummies.0[i];
        let ctx = crate::combat::SelectCtx {
            item: "PC_Sword",
            hits: player.combat.combo,
            items: ITEMS,
            finisher,
            only: if !finisher && target.is_some_and(|i| dummies.0[i].stun > 0.0 && dummies.0[i].quick <= 0.0 && dummies.0[i].down <= 0.0) { "PC_PoolAttacks_KD" } else { "" },
            target: Some(crate::combat::TargetCtx {
                distance_cm: d * 100.0,
                side: crate::combat::DIRS
                    .into_iter()
                    .min_by(|a, b| shortest_angle(side, a.side()).abs().total_cmp(&shortest_angle(side, b.side()).abs()))
                    .unwrap_or(crate::combat::Dir::Front),
                quick_stunned: dn.quick > 0.0,
                stunned: dn.stun > 0.0,
                knocked_down: dn.down > 0.0,
                parry_open: false,
                health: dn.hp,
            }),
        };
        let variety = player.combat.variety;
        if finisher {
            let pick = player.catalog.moves.select("Light", &ctx, player.combat.flurry as usize).cloned();
            debug!("finisher pick: {:?} (dist {:.2} m)", pick.as_ref().map(|p| p.name.clone()), d);
            if let Some(def) = pick.filter(|d| d.node.starts_with("PC_Flurry_Standing")) {
                if def.node.starts_with("PC_Flurry_Standing_END") {
                    player.combat.flurry = 0;
                    let victim = def.victim_clip.clone().unwrap_or_else(|| "Bip_PL_Sync_Flurry_Ender_Kill_F".into());
                    let front = !def.clip.ends_with("_B") && !def.clip.contains("_B_");
                    let orc_pos = dummies.0[i].pos;
                    if begin_sync(player, lib, counters, i, front, &def.clip, victim, crate::orc::Place::Facing, false) {
                        if front {
                            player.yaw = (-(orc_pos.x - player.pos.x)).atan2(-(orc_pos.z - player.pos.z));
                        }
                        return;
                    }
                } else if let Some(attack) = attack_from_def(lib, &def, &stance) {
                    player.combat.flurry += 1;
                    node_attack = Some(attack);
                }
            }
        } else {
            let pick = if dn.down > 0.0 {
                player.catalog.moves.select("Heavy", &ctx, variety).cloned()
            } else if dash {
                player.catalog.moves.select("Dash_Attack", &ctx, variety).cloned()
            } else if dn.stun > 0.0 && dn.quick <= 0.0 {
                player.catalog.moves.select("Light", &ctx, variety).cloned()
            } else {
                None
            };
            debug!("node pick (stun {:.1} down {:.1} dash {dash}): {:?}", dn.stun, dn.down, pick.as_ref().map(|p| (p.name.clone(), p.clip.clone())));
            node_attack = pick.and_then(|def| attack_from_def(lib, &def, &stance));
        }
    }
    if finisher && node_attack.is_none() {
        // F with nothing to finish does nothing.
        return;
    }
    if !finisher {
        player.combat.flurry = 0;
    }
    let Some(attack) = node_attack.or_else(|| {
        let recent = player.combat.recent.clone();
        let seed = player.combat.variety as u32 ^ (player.pos.x * 977.0 + player.pos.z * 131.0) as i32 as u32;
        player.catalog.choose(lib, player.combat.combo, side, distance, &stance, &recent, seed).cloned()
    }) else {
        warn!("no attack clip for {} hits", player.combat.combo);
        return;
    };
    // Aim the clip: turn the remaining angle so its authored side faces the target, and stretch its travel
    // so it ends at the stand-off distance.
    player.combat.pending_turn = if target.is_some() { shortest_angle(attack.dir.side(), side) } else { 0.0 };
    player.combat.warp = match distance {
        Some(d) if attack.strike_reach > 0.25 => ((d - STAND_OFF) / attack.strike_reach).clamp(0.25, 2.2),
        _ => 1.0,
    };
    // Timing comes from the clip's own cues.
    let seconds = lib.duration_ms(attack.clip) / 1000.0;
    player.combat.hit_s = lib.event_s(attack.clip, "APPLYDAMAGE").unwrap_or(seconds * 0.5);
    player.combat.chain_s = lib
        .event_s(attack.clip, "BUFFTRANS")
        .or_else(|| lib.event_s(attack.clip, "EHITWIN"))
        .unwrap_or(seconds * 0.7)
        .min(seconds);
    player.combat.hit_done = false;
    player.combat.blade_gap = f32::MAX;
    player.combat.landed_at = None;
    player.combat.target = target;
    // Only an attack authored for a target in front, that does not spin, is kept aimed at it; kicks to the side or behind and
    // turning cuts carry their own facing.
    player.combat.steer = attack.dir == crate::combat::Dir::Front && lib.root_total(attack.clip).map_or(true, |(_, yaw)| yaw.abs() < 0.5);
    player.combat.blade_min = (f32::MAX, 0.0);
    player.combat.queued = false;
    player.combat.last_attack = lib.name(attack.clip).to_string();
    player.combat.applied = player.catalog.moves.applied(&attack.name).to_vec();
    player.combat.leg = attack.name.contains("LegSweep");
    player.combat.recent.push(player.combat.last_attack.clone());
    if player.combat.recent.len() > 6 {
        player.combat.recent.remove(0);
    }
    player.combat.stance = attack.end.clone();
    info!("attack {} (hits {}, {} -> {}{})", player.combat.last_attack, player.combat.combo, attack.start, attack.end, if attack.heavy { ", heavy" } else { "" });
    player.combat.cancel_s = lib.event_s(attack.clip, "BCANWIN").unwrap_or(0.0);
    player.combat.collide = (lib.event_s(attack.clip, "BCOL").unwrap_or(0.0), lib.event_s(attack.clip, "ECOL").unwrap_or(seconds));
    player.combat.face = lib.event_s(attack.clip, "STARTFACING").zip(lib.event_s(attack.clip, "STOPFACING"));
    player.combat.heavy = attack.heavy;
    player.combat.swing = attack.swing;
    player.state = Loco::Attack { level: attack.tier };
    if let Err(err) = player.animator.restart(lib, attack.clip, false, fade, 0.0) {
        error_once!("animation failed: {err:#}");
    }
}

#[allow(clippy::too_many_arguments)]
pub fn drive_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    options: Res<Options>,
    rig: Option<Res<Rig>>,
    player: Option<ResMut<Player>>,
    mut dummies: ResMut<Dummies>,
    mut blows: ResMut<crate::orc::IncomingBlows>,
    mut counters: ResMut<crate::orc::CounterState>,
    mut stop: ResMut<crate::combat::HitStop>,
    mut fx: ResMut<crate::combat::ImpactFx>,
    cameras: Query<&FollowCamera>,
    geometry: Res<crate::world::WorldGeometry>,
    mut transforms: Query<&mut Transform>,
) {
    let (Some(rig), Some(mut player)) = (rig, player) else { return };
    let dt = time.delta_secs().min(0.05);
    let camera_yaw = cameras.iter().next().map_or(0.0, |c| c.yaw);
    let (dir, walk) = read_input(&options, &keys, &time, camera_yaw);
    let moving_input = dir != Vec3::ZERO;
    let run = !walk;
    let edge = |held: bool, down: bool| (down && !held, down);
    let (roll_pressed, down) = edge(player.roll_held, options.pressed(&keys, &time, KeyCode::Space));
    player.roll_held = down;
    let _attack_held_now = player.attack_held;
    let (attack_pressed, down) =
        edge(player.attack_held, mouse.pressed(MouseButton::Left) || options.pressed(&keys, &time, KeyCode::KeyL));
    player.attack_held = down;
    // F: the finisher, only while an orc in reach is stunned.
    let (finisher_pressed, down) = edge(player.finisher_held, options.pressed(&keys, &time, KeyCode::KeyF));
    player.finisher_held = down;
    if finisher_pressed {
    }
    // The charge follows the hit counter: gained at every multiple of `STREAK`, lost with the streak.
    if player.combat.combo == 0 {
        player.combat.charged = false;
    }
    let execute_pressed = finisher_pressed && player.combat.charged && !matches!(player.state, Loco::Counter | Loco::Hurt );
    let (counter_pressed, down) =
        edge(player.counter_held, mouse.pressed(MouseButton::Right) || options.pressed(&keys, &time, KeyCode::KeyQ));
    player.counter_held = down;
    let (crouch_pressed, down) = edge(
        player.crouch_held,
        options.pressed(&keys, &time, KeyCode::KeyC) || options.pressed(&keys, &time, KeyCode::ControlLeft),
    );
    player.crouch_held = down;
    let (hurt_pressed, down) = edge(player.hurt_held, options.pressed(&keys, &time, KeyCode::KeyH));
    player.hurt_held = down;
    let lib = &rig.lib;
    {
        let facing = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
        let engaged = player.combat.drawn || matches!(player.state, Loco::Attack { .. } | Loco::Roll | Loco::Counter);
        let pos = player.pos;
        dummies.update_lock(pos, facing, dir, engaged);
    }
    // Execution (the game's `PC_InstaKill_KBM` node, input `TokenKill_KBM`): a standing kill, or a ground kill on a downed orc.
    if execute_pressed {
        let facing = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
        let target = dummies.1.filter(|&i| dummies.0[i].hp > 0.0).or_else(|| dummies.nearest(player.pos, facing, 4.0, std::f32::consts::PI));
        if let Some(i) = target.filter(|&i| (dummies.0[i].pos - player.pos).length() < 4.0) {
            let dn = &dummies.0[i];
            let orc_facing = Vec3::new(-dn.yaw.sin(), 0.0, -dn.yaw.cos());
            let in_front = orc_facing.dot((player.pos - dn.pos).normalize_or_zero()) >= 0.0;
            let down = dn.down > 0.0;
            let order = if in_front {
                [crate::combat::Dir::Front, crate::combat::Dir::Left, crate::combat::Dir::Right, crate::combat::Dir::Back]
            } else {
                [crate::combat::Dir::Back, crate::combat::Dir::Left, crate::combat::Dir::Right, crate::combat::Dir::Front]
            };
            let variety = player.combat.variety;
            let pick = order.into_iter().find_map(|side| {
                let ctx = crate::combat::SelectCtx {
                    item: "PC_Sword",
                    hits: player.combat.combo,
                    items: ITEMS,
                    finisher: false,
                    only: "",
                    target: Some(crate::combat::TargetCtx {
                        distance_cm: 100.0,
                        side,
                        quick_stunned: false,
                        stunned: dn.stun > 0.0 && !down,
                        knocked_down: down,
                        parry_open: false,
                        health: dn.hp,
                    }),
                };
                player.catalog.moves.select("TokenKill_KBM", &ctx, variety).cloned().filter(|d| lib.find(&d.clip).is_some()).map(|d| (side, d))
            });
            if let Some((side, def)) = pick {
                let front = side != crate::combat::Dir::Back;
                let victim = def.victim_clip.clone().unwrap_or_default();
                let orc_pos = dn.pos;
                let place = if front { crate::orc::Place::Facing } else { crate::orc::Place::SameWay };
                if begin_sync(&mut player, lib, &mut counters, i, front, &def.clip, victim, place, false) {
                    player.yaw = (-(orc_pos.x - player.pos.x)).atan2(-(orc_pos.z - player.pos.z));
                    player.combat.charged = false;
                    player.combat.variety += 1;
                    info!("execution {} ({})", def.name, def.clip);
                }
            }
        }
    }
    let wanted = if moving_input { (-dir.x).atan2(-dir.z) } else { player.yaw };
    let angle = shortest_angle(player.yaw, wanted);
    let deg = angle.abs().to_degrees();

    let play = |player: &mut Player, clip: ClipId, looped: bool, fade: f32, phase: f32| {
        if let Err(err) = player.animator.play(lib, clip, looped, fade, phase) {
            error_once!("animation failed: {err:#}");
        }
    };
    let go_idle = |player: &mut Player| {
        player.state = Loco::Idle;
        let idle = player.clips.idle;
        play(player, idle, true, 0.3, 0.0);
    };
    let go_moving = |player: &mut Player, run: bool, fade: f32, phase: f32| {
        player.state = Loco::Moving { run };
        player.gait_time = 0.0;
        let (clip, rate) = gait(player, lib, run).unwrap_or((if run { player.clips.run } else { player.clips.walk }, 1.0));
        player.gait_rate = rate;
        play(player, clip, true, fade, phase);
    };
    let go_stopping = |player: &mut Player, run: bool| {
        // The graph's stops: `RunToIdle_L/R` (which foot is down is read from the cycle, approximately) and `Walking_To_Idle`.
        let foot = player.animator.phase(lib) >= 0.5;
        player.state = Loco::Stopping;
        let state = if run { if foot { "RunToIdle_R" } else { "RunToIdle_L" } } else { "Walking_To_Idle" };
        let ctx = crate::gait::GroundCtx { stealth: player.crouching, in_combat: player.combat.calm < COMBAT_MEMORY, node_time: 0.0, move_mag: 0.0 };
        let (clip, rate) = player.gaits.as_ref().and_then(|g| g.steady(state, ctx, lib)).unwrap_or((if run { player.clips.stop_run } else { player.clips.stop_walk }, 1.0));
        player.gait_rate = rate;
        play(player, clip, false, 0.15, 0.0);
    };
    let elapsed = player.animator.elapsed_s(lib);
    let free = matches!(
        player.state,
        Loco::Idle | Loco::Starting { .. } | Loco::Moving { .. } | Loco::Stopping | Loco::Turning { .. }
    );
    let recovering = matches!(player.state, Loco::Attack { level } if level > 0) && elapsed >= player.combat.cancel_s;

    // Blows from orcs: a roll evades them, a counter ignores them, anything else hurts.
    for blow in std::mem::take(&mut blows.0) {
        match player.state {
            Loco::Roll if elapsed < 0.75 => {}
            Loco::Counter => {}
            _ => {
                player.hp -= blow.damage;
                if player.hp <= 0.0 {
                    player.hp = 100.0;
                }
                player.combat.combo = 0;
                player.crouching = false;
                player.clips = player.standing;
                let options: Vec<ClipId> = player.catalog.recoils.iter().filter(|r| r.1 == "light").map(|r| r.0).collect();
                let pick = player.combat.hurt_cycle % options.len().max(1);
                if let Some(&clip) = options.get(pick) {
                    player.combat.hurt_cycle += 1;
                    player.combat.drawn = true;
                    player.combat.calm = 0.0;
                    player.state = Loco::Hurt;
                    play(&mut player, clip, false, 0.04, 0.0);
                    stop.0 = stop.0.max(0.08);
                    fx.0.push((blow.point, false));
                }
            }
        }
    }
    let was_crouching = player.crouching;
    // Crouch (stealth) toggles while on his feet; fighting stands him up.
    let fighting = matches!(player.state, Loco::Attack { .. } | Loco::Roll | Loco::Counter | Loco::Hurt);
    let standing_up = fighting || attack_pressed || roll_pressed || counter_pressed;
    if (crouch_pressed && matches!(player.state, Loco::Idle | Loco::Moving { .. } | Loco::Stopping)) || (player.crouching && standing_up) {
        player.crouching = !player.crouching && !standing_up;
        player.clips = if player.crouching { player.crouched } else { player.standing };
        if matches!(player.state, Loco::Idle | Loco::Stopping) {
            go_idle(&mut player);
        } else if let Loco::Moving { run } = player.state {
            go_moving(&mut player, run, 0.3, 0.0);
        }
    }
    // Combat entry: counter, roll, attack, get hurt.
    // The counter prompt is up: the counter key (or a tap of attack) answers it.
    let prompted = counters.prompt.filter(|_| free || recovering);
    let counter_ctx = |pos: Vec3, parry_open: bool| {
        let to = Vec3::new(pos.x - player.pos.x, 0.0, pos.z - player.pos.z);
        let facing = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
        let front = facing.dot(to.normalize_or_zero()) >= 0.0;
        (
            front,
            crate::combat::TargetCtx {
                distance_cm: to.length() * 100.0,
                side: if front { crate::combat::Dir::Front } else { crate::combat::Dir::Back },
                quick_stunned: false,
                stunned: false,
                knocked_down: false,
                parry_open,
                health: 100.0,
            },
        )
    };
    let cycle = std::cell::Cell::new(player.combat.counter_cycle);
    let counter_pick = prompted.filter(|_| counter_pressed || attack_pressed).and_then(|orc| {
        let (front, target) = counter_ctx(dummies.0[orc].pos, true);
        let ctx = crate::combat::SelectCtx { item: "PC_Sword", hits: player.combat.combo, items: ITEMS, target: Some(target), finisher: false, only: "" };
        let variety = player.combat.variety + 1;
        // The game's counter families: the basic counter (hit counter 0-8), then the stun / knock-down / knock-back outcomes.
        // Which one a given prompt asks for is not decoded here, so they take turns.
        const FAMILIES: [&str; 4] = ["Counter", "Counter_Stun", "Counter_Knockdown", "Counter_Knockback"];
        let first = cycle.get();
        let mut pick = None;
        for k in 0..FAMILIES.len() {
            pick = player.catalog.moves.select(FAMILIES[(first + k) % FAMILIES.len()], &ctx, variety).cloned();
            if pick.is_some() {
                cycle.set((first + k + 1) % FAMILIES.len());
                break;
            }
        }
        pick.map(|def| (orc, front, def))
    });
    player.combat.counter_cycle = cycle.get();
    if let Some((orc, front, def)) = counter_pick {
        player.combat.variety += 1;
        let victim = def.victim_clip.clone().unwrap_or_default();
        if front {
            let to = dummies.0[orc].pos - player.pos;
            player.yaw = (-to.x).atan2(-to.z);
        }
        if begin_sync(&mut player, lib, &mut counters, orc, front, &def.clip, victim, crate::orc::Place::Facing, false) {
            // Only the kill counter finishes the orc; the others (the basic counter included) leave it stunned / knocked down.
            player.combat.sync_lethal = def.node == "PC_Counter_Kill";
            player.combat.sync_applied = def.applied.clone();
            info!("counter {} ({}) lethal {}", def.name, def.node, player.combat.sync_lethal);
        }
    } else if counter_pressed && free && prompted.is_none() {
        // No parry window to answer: the game's `PC_Counter_Fail` node (the lowest priority Counter node) plays `Elf_ParryFail`.
        let ctx = crate::combat::SelectCtx { item: "PC_Sword", hits: player.combat.combo, items: ITEMS, target: None, finisher: false, only: "" };
        let fail = player.catalog.moves.select("Counter", &ctx, 0).cloned();
        if let Some(clip) = fail.and_then(|d| lib.find(&d.clip)) {
            player.combat.drawn = true;
            player.combat.calm = 0.0;
            player.state = Loco::Hurt;
            play(&mut player, clip, false, 0.08, 0.0);
        }
    } else if roll_pressed && (free || recovering) && {
        // Rolling into an orc vaults over it (the game's `PC_DashOver` node, same `Dash` input as the roll).
        let facing = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
        let heading = if moving_input { dir } else { facing };
        let target = dummies.nearest(player.pos, heading, 2.0, 50f32.to_radians());
        let pick = target.and_then(|i| {
            let dn = &dummies.0[i];
            let orc_facing = Vec3::new(-dn.yaw.sin(), 0.0, -dn.yaw.cos());
            let in_front = orc_facing.dot((player.pos - dn.pos).normalize_or_zero()) >= 0.0;
            let ctx = crate::combat::SelectCtx {
                item: "PC_Sword",
                hits: player.combat.combo,
                items: ITEMS,
                finisher: false,
                only: "PC_DashOver",
                target: Some(crate::combat::TargetCtx {
                    distance_cm: (dn.pos - player.pos).length() * 100.0,
                    side: if in_front { crate::combat::Dir::Front } else { crate::combat::Dir::Back },
                    quick_stunned: dn.quick > 0.0,
                    stunned: dn.stun > 0.0,
                    knocked_down: dn.down > 0.0,
                    parry_open: false,
                    health: dn.hp,
                }),
            };
            player.catalog.moves.select("Dash", &ctx, 0).cloned().filter(|d| lib.find(&d.clip).is_some()).map(|d| (i, in_front, d))
        });
        match pick {
            Some((i, in_front, def)) => {
                let orc_pos = dummies.0[i].pos;
                let victim = def.victim_clip.clone().unwrap_or_default();
                let place = if in_front { crate::orc::Place::Facing } else { crate::orc::Place::SameWay };
                if begin_sync(&mut player, lib, &mut counters, i, in_front, &def.clip, victim, place, false) {
                    player.yaw = (-(orc_pos.x - player.pos.x)).atan2(-(orc_pos.z - player.pos.z));
                    player.combat.sync_lethal = false;
                    player.combat.sync_applied = def.applied.clone();
                    player.combat.sync_harmless = true;
                    info!("vault {} ({})", def.name, def.clip);
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    } {
    } else if roll_pressed && (free || recovering) {
        if let Some(clip) = player.catalog.roll {
            // Roll the way the stick points, or the way he faces.
            player.combat.pending_turn = if moving_input { angle } else { 0.0 };
            player.combat.warp = 1.0;
            player.combat.drawn = true;
            player.combat.calm = 0.0;
            player.state = Loco::Roll;
            play(&mut player, clip, false, 0.08, 0.0);
        }
    } else if attack_pressed && free && was_crouching && {
        // A crouched strike on an orc that has not noticed him.
        let facing = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
        dummies.nearest(player.pos, facing, 2.6, 80f32.to_radians()).is_some_and(|i| !dummies.0[i].aware)
    } {
        let facing = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
        let i = dummies.nearest(player.pos, facing, 2.6, 80f32.to_radians()).unwrap_or(0);
        let orc_pos = dummies.0[i].pos;
        let to_player = Vec3::new(player.pos.x - orc_pos.x, 0.0, player.pos.z - orc_pos.z).normalize_or_zero();
        let orc_facing = Vec3::new(-dummies.0[i].yaw.sin(), 0.0, -dummies.0[i].yaw.cos());
        let behind = orc_facing.dot(to_player) < 0.0;
        player.yaw = (-(orc_pos.x - player.pos.x)).atan2(-(orc_pos.z - player.pos.z));
        player.combat.variety += 1;
        let kinds: &[&str] = if behind { &["B_SkullStab", "B_StabNeckBreak", "B_StabOpen"] } else { &["F_TakeDownStab"] };
        let kind = kinds[player.combat.variety % kinds.len()];
        let place = if behind { crate::orc::Place::SameWay } else { crate::orc::Place::Facing };
        begin_sync(
            &mut player,
            lib,
            &mut counters,
            i,
            !behind,
            &format!("PL_Bip_Sync_StealthKill_{kind}"),
            format!("Bip_PL_Sync_StealthKill_{kind}"),
            place,
            true,
        );
    } else if attack_pressed && free {
        // Sprinting into an attack is the dash attack.
        player.combat.dash_next = matches!(player.state, Loco::Moving { run: true }) && player.speed > 3.0;
        begin_attack(&mut player, lib, &dummies, &mut counters, true, dir, 0.12);
    } else if hurt_pressed {
        player.combat.combo = 0;
        let weights = ["light", "medium", "heavy"];
        let weight = weights[player.combat.hurt_cycle % 3];
        let options: Vec<ClipId> = player.catalog.recoils.iter().filter(|r| r.1 == weight).map(|r| r.0).collect();
        let pick = player.combat.hurt_cycle / 3 % options.len().max(1);
        player.combat.hurt_cycle += 1;
        if let Some(&clip) = options.get(pick) {
            player.state = Loco::Hurt;
            play(&mut player, clip, false, 0.06, 0.0);
        }
    }

    // Combat entry may have started a new clip this frame; read its clock, not the old clip's.
    let elapsed = player.animator.elapsed_s(lib);
    let ended = player.animator.ended();

    match player.state {
        Loco::Idle if moving_input => {
            let (clip, turning) = match start_turn(&player.clips, run, angle) {
                Some(clip) => (clip, true),
                None => (if run { player.clips.start_run } else { player.clips.start_walk }, false),
            };
            player.state = Loco::Starting { run, turning };
            play(&mut player, clip, false, 0.12, 0.0);
        }
        Loco::Starting { run: was_run, turning } => {
            if !moving_input && elapsed > 0.25 && !turning {
                go_stopping(&mut player, was_run);
            } else if ended {
                go_moving(&mut player, run, 0.2, 0.0);
            }
        }
        Loco::Moving { run: was_run } => {
            player.gait_time += dt;
            // The tree follows `Game_NodeLifeTime` (`Sprint`: Hustle85 until 1.5 s, then Sprint85): a new branch cross-fades in at the same phase.
            if moving_input && was_run == run {
                if let Some((clip, rate)) = gait(&player, lib, run) {
                    player.gait_rate = rate;
                    if player.animator.current() != Some(clip) {
                        let phase = player.animator.phase(lib);
                        play(&mut player, clip, true, 0.3, phase);
                    }
                }
            }
            if !moving_input {
                go_stopping(&mut player, was_run);
            } else if was_run && deg > 150.0 {
                player.state = Loco::Turning { run: true, then_stop: false };
                let clip = player.clips.run_turn_180.pick(angle);
                play(&mut player, clip, false, 0.15, 0.0);
            } else if was_run && deg > 70.0 {
                player.state = Loco::Turning { run: true, then_stop: false };
                let clip = player.clips.run_pivot_90.pick(angle);
                play(&mut player, clip, false, 0.15, 0.0);
            } else if !was_run && deg > 150.0 {
                player.state = Loco::Turning { run: false, then_stop: true };
                let foot = (player.animator.phase(lib) >= 0.5) as usize;
                let clip = player.clips.walk_turn_180[foot].pick(angle);
                play(&mut player, clip, false, 0.15, 0.0);
            } else if !was_run && deg > 50.0 {
                player.state = Loco::Turning { run: false, then_stop: false };
                let clip = player.clips.walk_turn_out.pick(angle);
                play(&mut player, clip, false, 0.15, 0.0);
            } else if was_run != run {
                // Switching gait: keep the cycle phase so the feet do not pop.
                let phase = player.animator.phase(lib);
                go_moving(&mut player, run, 0.3, phase);
            }
        }
        Loco::Turning { run: was_run, then_stop } => {
            if ended {
                if then_stop || !moving_input {
                    go_idle(&mut player);
                } else {
                    go_moving(&mut player, was_run && run, 0.2, 0.0);
                }
            }
        }
        Loco::Stopping => {
            if moving_input && elapsed > 0.3 {
                go_moving(&mut player, run, 0.25, 0.0);
            } else if ended {
                go_idle(&mut player);
            }
        }
        Loco::Attack { level } => {
            if attack_pressed && elapsed > 0.15 {
                player.combat.queued = true;
            }
            let chain = player.combat.chain_s;
            // Keep swinging while the button is queued; otherwise recover into idle or movement.
            if level == 0 {
                if ended {
                    begin_attack(&mut player, lib, &dummies, &mut counters, false, dir, 0.1);
                }
            } else if player.combat.queued && (elapsed >= chain || player.combat.landed_at.is_some_and(|t| elapsed >= t + FOLLOW_THROUGH)) {
                // Freeflow: the cancel window (`BCANWIN`) is open from the first frame of the pool attacks, so once the blow has
                // landed a buffered attack cuts in at once; `BUFFTRANS` is only where it goes when nothing was hit.
                begin_attack(&mut player, lib, &dummies, &mut counters, false, dir, 0.12);
            } else if moving_input && (elapsed >= chain || player.combat.landed_at.is_some_and(|t| elapsed >= t + FOLLOW_THROUGH * 2.0)) {
                // Once the blow has landed the stick carries him straight on into the gait instead of waiting for the clip's tail.
                go_moving(&mut player, run, 0.2, 0.0);
            } else if ended {
                go_idle(&mut player);
            }
        }
        Loco::Counter => {
            // The blade goes home late in the clip; the orc's own clip plays the fall.
            let kill_at = player.combat.sync_kill;
            if let Some((orc, false)) = player.combat.counter {
                if elapsed >= kill_at && player.combat.sync_harmless {
                    player.combat.counter = Some((orc, true));
                } else if elapsed >= kill_at {
                    player.combat.counter = Some((orc, true));
                    if player.combat.sync_lethal {
                        if let Some(d) = dummies.0.get_mut(orc) {
                            d.hp = 0.0;
                            d.hits += 1;
                            d.flash = 1.0;
                            fx.0.push((Vec3::new(d.pos.x, 1.2, d.pos.z), true));
                        }
                    } else if orc < dummies.0.len() {
                        // Stun / knock-down / knock-back counter: the orc survives with the state the category applies.
                        let applied = player.combat.sync_applied.clone();
                        let (at, from) = (dummies.0[orc].pos, player.pos);
                        dummies.strike(orc, from, 0.0, 6.0, "ST", false, false, &applied, player.combat.combo, false);
                        dummies.0[orc].hit = None;
                        fx.0.push((Vec3::new(at.x, 1.2, at.z), false));
                    }
                    stop.0 = stop.0.max(0.14);
                    player.combat.combo += 1;
                    if player.combat.combo % STREAK == 0 {
                        player.combat.charged = true;
                    }
                    player.combat.since_hit = 0.0;
                }
            }
            if ended {
                player.combat.counter = None;
                player.combat.dagger = false;
                go_idle(&mut player);
            }
        }
        Loco::Hurt => {
            if ended {
                go_idle(&mut player);
            }
        }
        Loco::Roll => {
            if ended || (moving_input && elapsed > 0.75) {
                if moving_input {
                    go_moving(&mut player, run, 0.2, 0.0);
                } else {
                    go_idle(&mut player);
                }
            }
        }
        Loco::Idle => {}
    }

    // Steering: gentle while a turning clip is rotating the character, full otherwise.
    let rate = match player.state {
        Loco::Idle | Loco::Starting { turning: false, .. } => TURN_START,
        Loco::Starting { turning: true, .. } | Loco::Turning { .. } => TURN_CORRECT,
        Loco::Moving { .. } => TURN_MOVING,
        Loco::Stopping
        | Loco::Attack { .. }
        | Loco::Counter
        | Loco::Hurt
        | Loco::Roll
        => 0.0,
    };
    if moving_input {
        let diff = shortest_angle(player.yaw, wanted);
        player.yaw += diff.clamp(-rate * dt, rate * dt);
    }
    // Line an attack (or a roll) up with its direction over its first moments.
    if matches!(player.state, Loco::Attack { .. } | Loco::Roll) && player.combat.pending_turn.abs() > 1e-3 {
        let turn = player.combat.pending_turn * (dt * 9.0).min(1.0);
        player.yaw += turn;
        player.combat.pending_turn -= turn;
    }

    // Inside the clip's facing window the attack turns him toward the locked target.
    if let (Loco::Attack { level }, Some((from, to)), Some(i)) = (player.state, player.combat.face, dummies.1) {
        let now = player.animator.elapsed_s(lib);
        if level > 0 && now >= from && now <= to {
            let d = dummies.0[i].pos - player.pos;
            let diff = shortest_angle(player.yaw, (-d.x).atan2(-d.z));
            player.yaw += diff.clamp(-9.0 * dt, 9.0 * dt);
        }
    }

    // Animate; the root travel moves the entity.
    let rate = if matches!(player.state, Loco::Moving { .. } | Loco::Stopping) { player.gait_rate } else { 1.0 };
    let step = match player.animator.update(lib, dt * rate) {
        Ok(step) => step,
        Err(err) => {
            error_once!("animation failed: {err:#}");
            return;
        }
    };
    // Game space is Z-mirrored from Bevy, so a game yaw turns the other way.
    player.yaw -= step.yaw;
    // After the blow he is already at striking distance: the rest of the clip's travel would walk him through the orc.
    let mut warp = match player.state {
        Loco::Attack { level } if level > 0 && player.combat.landed_at.is_some() => player.combat.warp.min(0.25),
        Loco::Attack { .. } => player.combat.warp,
        _ => 1.0,
    };
    // Freeflow approach (the game's translation scaling, `BTS`..`ETS`): while the swing winds up he is carried to striking
    // distance of the target wherever it has moved to, arriving as the blade's collision window opens.
    let mut homing = Vec3::ZERO;
    if let (Loco::Attack { level }, false, Some(i)) = (player.state, player.combat.hit_done, player.combat.target) {
        if level > 0 && i < dummies.0.len() && dummies.0[i].hp > 0.0 {
            let now = player.animator.elapsed_s(lib);
            let arrive = player.combat.collide.0.max(0.12);
            let to = Vec3::new(dummies.0[i].pos.x - player.pos.x, 0.0, dummies.0[i].pos.z - player.pos.z);
            let gap = to.length() - STAND_OFF;
            if to.length() < 9.0 {
                warp = 0.0;
                if gap > 0.0 {
                    let left = (arrive - now).max(dt);
                    homing = to.normalize_or_zero() * (gap * (dt / left).min(1.0)).min(14.0 * dt);
                }
                // Keep the swing aimed at him.
                if player.combat.steer {
                    let want = (-to.x).atan2(-to.z) - player.combat.pending_turn;
                    player.yaw += shortest_angle(player.yaw, want).clamp(-10.0 * dt, 10.0 * dt);
                }
            }
        }
    }
    let local = to_bevy_vec([step.travel[0] * warp, 0.0, step.travel[2] * warp]);
    let world = Quat::from_rotation_y(player.yaw) * local + homing;
    let old = player.pos;
    player.pos.x = (player.pos.x + world.x).clamp(-PLATE_LIMIT, PLATE_LIMIT);
    player.pos.z = (player.pos.z + world.z).clamp(-PLATE_LIMIT, PLATE_LIMIT);
    // The arena floor is flat.
    if !matches!(player.state, Loco::Counter) {
        player.pos.y = geometry.ground(player.pos, 0.0);
    }
    // Walls stop him (a synchronised move is placed by its partner and is left alone).
    if !matches!(player.state, Loco::Counter) {
        player.pos = geometry.push_out(player.pos, crate::world::BODY_RADIUS);
    }
    player.speed = Vec2::new(player.pos.x - old.x, player.pos.z - old.z).length() / dt.max(1e-4);

    // The blow lands when the blade reaches the target inside the clip's collision window, or at the latest on
    // its `APPLYDAMAGE` cue.
    let elapsed_now = player.animator.elapsed_s(lib);
    if let Loco::Attack { level } = player.state {
        if level > 0 && !player.combat.hit_done {
            let facing = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
            let (from, to) = player.combat.collide;
            // Slow motion just before the blade connects: from 200 ms to 100 ms ahead of `BCOL`, then it ramps back out.
    // The blow lands when the weapon collider is live (`BCOL`..`ECOL`) and the target is within the blade's reach in
            // front of him - with the approach arriving as the window opens, that is the moment it opens (the game times its
            // hit-counter slow motion just before `BCOL`). `APPLYDAMAGE` / `AD` is the fallback.
            let mut contact = false;
            if elapsed_now >= from && elapsed_now <= to {
                contact = dummies.0.iter().any(|d| {
                    let to_orc = Vec3::new(d.pos.x - player.pos.x, 0.0, d.pos.z - player.pos.z);
                    // A kick behind or to the side hits where the clip aims it, not where he faces.
                    d.hp > 0.0 && to_orc.length() < STAND_OFF + 0.75 && (!player.combat.steer || to_orc.normalize_or_zero().dot(facing) > 0.3)
                });
            }
            if contact || elapsed_now >= player.combat.hit_s {
                player.combat.hit_done = true;
                info!("blow at {:.2}s ({}), cue {:.2}s, window {:.2}-{:.2}", elapsed_now, if contact { "contact" } else { "cue" }, player.combat.hit_s, from, to);
                if dummies.nearest(player.pos, facing, 3.2, 80f32.to_radians()).is_none() {
                    debug!("blow missed: orc at {:?}, player {:?} yaw {:.2}", dummies.0.iter().map(|d| d.pos).collect::<Vec<_>>(), player.pos, player.yaw);
                }
                // The orc the attack was aimed at, if it is still in reach; otherwise whoever is in front.
                let aimed = player.combat.target.filter(|&i| i < dummies.0.len() && dummies.0[i].hp > 0.0 && (dummies.0[i].pos - player.pos).length() < 3.2);
                if let Some(i) = aimed.or_else(|| dummies.nearest(player.pos, facing, 3.2, 80f32.to_radians())) {
                    let damage = DAMAGE[level as usize] + if player.combat.heavy { 12.0 } else { 0.0 };
                    let at = dummies.0[i].pos;
                    let applied = player.combat.applied.clone();
                    dummies.strike(i, player.pos, if player.combat.heavy { 3.4 } else { 2.4 }, damage, player.combat.swing, player.combat.heavy, player.combat.flurry == 0, &applied, player.combat.combo, player.combat.leg);
                    player.combat.landed_at = Some(elapsed_now);
                    player.combat.combo += 1;
                    if player.combat.combo % STREAK == 0 {
                        player.combat.charged = true;
                    }
                    player.combat.since_hit = 0.0;
                    stop.0 = stop.0.max(if player.combat.heavy { 0.12 } else { 0.07 });
                    let toward = (player.pos - at).normalize_or_zero() * 0.35;
                    fx.0.push((Vec3::new(at.x + toward.x, 1.25, at.z + toward.z), player.combat.heavy));
                }
            }
        }
    }
    // The combo (hit counter) runs out a few seconds after the last landed hit.
    player.combat.since_hit += dt;
    if player.combat.since_hit > COMBO_TIMEOUT {
        player.combat.combo = 0;
    }
    // Seconds since combat last happened (the gait trees' `InCombat` branch follows it).
    player.combat.calm += dt;

    if let Ok(mut transform) = transforms.get_mut(rig.player) {
        transform.translation = player.pos;
        transform.rotation = Quat::from_rotation_y(player.yaw);
    }
    for (joint, bone) in rig.joints.iter().zip(&step.pose) {
        if let Ok(mut transform) = transforms.get_mut(*joint) {
            transform.translation = to_bevy_vec(bone.t);
            transform.rotation = to_bevy_quat(bone.r);
        }
    }
}

/// Show the sword in his hand while in combat and the one on his back otherwise; the dagger moves from its belt
/// sheath to his hand for stealth kills.
fn update_sword(
    mut commands: Commands,
    player: Option<Res<Player>>,
    rig: Option<Res<Rig>>,
    mut visibilities: Query<&mut Visibility>,
    mut in_hand: Local<bool>,
) {
    let (Some(player), Some(rig)) = (player, rig) else { return };
    let dagger_out = player.combat.dagger || std::env::var_os("SOM_DAGGER").is_some();
    let drawn = player.combat.drawn && !dagger_out;
    if let Ok(mut v) = visibilities.get_mut(rig.sword) {
        *v = if drawn { Visibility::Inherited } else { Visibility::Hidden };
    }
    for part in &rig.back_sword {
        if let Ok(mut v) = visibilities.get_mut(*part) {
            *v = if drawn { Visibility::Hidden } else { Visibility::Inherited };
        }
    }
    if dagger_out != *in_hand {
        *in_hand = dagger_out;
        commands.entity(rig.dagger).insert(ChildOf(if dagger_out { rig.dagger_hand } else { rig.dagger_sheath }));
    }
}

/// Once the orcs are loaded, forget the synchronised moves they have no clip for.
fn prune_sync_moves(player: Option<ResMut<Player>>, orcs: Option<Res<crate::orc::Orcs>>, mut done: Local<bool>) {
    if *done {
        return;
    }
    if let (Some(mut player), Some(orcs)) = (player, orcs) {
        player.prune_moves(|name| orcs.has_clip(name));
        *done = true;
    }
}

fn spawn_hud(mut commands: Commands) {
    // The hit counter, top left, as the game shows it: a white streak with `xN` on it, red once the Hit Streak is charged.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(214.0),
            left: Val::Px(0.0),
            width: Val::Px(230.0),
            height: Val::Px(5.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.85, 0.95, 1.0, 0.0)),
        ComboStreak,
    ));
    commands.spawn((
        Text::new(""),
        TextFont { font_size: bevy::text::FontSize::Px(44.0), ..default() },
        TextColor(Color::srgb(0.95, 0.95, 0.95)),
        Node { position_type: PositionType::Absolute, top: Val::Px(184.0), left: Val::Px(26.0), ..default() },
        ComboHud,
    ));
}

#[derive(Component)]
struct ComboHud;

#[derive(Component)]
struct ComboStreak;

fn update_combo_hud(
    player: Option<Res<Player>>,
    mut hud: Query<(&mut Text, &mut TextColor, &mut TextFont), With<ComboHud>>,
    mut streak: Query<&mut BackgroundColor, With<ComboStreak>>,
) {
    let Ok((mut text, mut color, mut font)) = hud.single_mut() else { return };
    let Some(p) = player else { return };
    let combo = p.combat.combo;
    text.0 = if combo > 0 { format!("x{combo}") } else { String::new() };
    // Each hit pops the number; it fades as the streak is about to run out.
    let pop = (1.0 - p.combat.since_hit / 0.22).clamp(0.0, 1.0);
    let fade = (1.0 - (p.combat.since_hit - (COMBO_TIMEOUT - 1.0)).max(0.0)).clamp(0.0, 1.0);
    font.font_size = bevy::text::FontSize::Px(44.0 + 20.0 * pop);
    color.0 = if p.combat.charged { Color::srgba(1.0, 0.16, 0.08, fade) } else { Color::srgba(0.96, 0.96, 0.96, fade) };
    if let Ok(mut bar) = streak.single_mut() {
        let glow = if combo > 0 { (0.35 + 0.65 * pop) * fade } else { 0.0 };
        bar.0 = Color::srgba(0.8, 0.93, 1.0, glow);
    }
}

