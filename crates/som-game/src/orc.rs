//! Orcs: the game's own orc models, skeletons, materials and animation banks, with a small brain.
//!
//! Each orc is one entry of [`crate::combat::Dummies`] (position, health, lock-on) plus an [`Orc`] here (the
//! skinned model and its animator). Reactions are picked from the game's recoil sets by the direction and
//! kind of the blow: `Orc_Recoil_<F|B>_<swing>_<100|200>cm`, and `OrcInf_Recoil_Heavy_<swing>_<F|B>` for
//! heavy strikes and kills.
//!
//! For now the orcs are training dummies: they stand and watch, and only swing when provoked (`T`), so the
//! counter can be tried. `SOM_HOSTILE=1` makes them fight back on their own.

use bevy::prelude::*;
use som_formats::animator::{Animator, ClipId, Library};
use som_formats::skel::Skeleton;
use som_formats::vfs::Vfs;

use crate::character::{to_bevy_quat, to_bevy_vec};
use crate::combat::Dummies;

/// The orc's animation banks (it uses the same generic stick figure as the player).
const BANKS: &[&str] = som_formats::banks::ORC;

/// Counter variants (the clip name suffix), for an attacker in front of and behind the player.
pub const COUNTER_FRONT: [&str; 6] =
    ["F", "F_9_TwoHandedSingle", "F_13_AngleOnePass", "F_17_TwoHandedDouble", "F_3_Shove", "F_4_Shove"];
pub const COUNTER_BACK: [&str; 3] = ["B", "B_3_Shove", "B_4_Shove"];

const ATTACK_RANGE: f32 = 1.7;
const NOTICE_RANGE: f32 = 13.0;
const ORC_DAMAGE: f32 = 12.0;

/// A blow an orc lands on the player; consumed by the player's controller.
pub struct Blow {
    pub damage: f32,
    pub point: Vec3,
}

#[derive(Resource, Default)]
pub struct IncomingBlows(pub Vec<Blow>);

/// The counter prompt shown over an attacking orc, and the player's answer to it.
#[derive(Resource, Default)]
pub struct CounterState {
    /// An orc is winding up a blow the player may counter.
    pub prompt: Option<usize>,
    pub request: Option<CounterRequest>,
}

/// A synchronised action (counter, finisher, stealth kill) the player starts on an orc.
pub struct CounterRequest {
    pub orc: usize,
    /// The orc is in front of the player.
    pub front: bool,
    /// The orc's half of the pair.
    pub victim: String,
    pub place: Place,
}

/// How the orc is put relative to the player when a sync action starts.
#[derive(Clone, Copy)]
pub enum Place {
    /// Facing the player, about a metre away (in front, or behind when `front` is false).
    Facing,
    /// A metre ahead of the player, both facing the same way (a kill from behind).
    SameWay,
}

/// How the last hit on an orc came in.
#[derive(Clone, Debug)]
pub struct Hit {
    pub from: Vec3,
    /// Swing code of the attacker's blow: `R2L`, `L2R`, `U2D`, `D2U` or `ST`.
    pub swing: &'static str,
    pub heavy: bool,
    pub kill: bool,
    /// What the blow did to the orc's state (from the attack's applied states).
    pub result: HitResult,
    /// A leg sweep (its own recoil clips).
    pub leg: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HitResult {
    Plain,
    Stun,
    Down,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Brain {
    Idle,
    Chase,
    Attack { hit_done: bool },
    Recoil { kill: bool },
    /// Dazed (`Stunned` / `Quick_Stunned`): the stunned loop until the timers run out, then the recovery.
    Stunned,
    /// Knocked down: the lying loop, then the get-up.
    Down,
    /// Playing a one-shot recovery (stun recover / get-up) before returning to normal.
    Recover,
    /// Playing the orc half of a counter.
    Sync,
    Dead,
}

struct Orc {
    root: Entity,
    /// The joint entities of each body piece (every piece carries the whole skeleton).
    pieces: Vec<Vec<Entity>>,
    animator: Animator,
    brain: Brain,
    yaw: f32,
    /// Seconds until it may attack again.
    cooldown: f32,
    variety: usize,
    /// Set by `T`: this orc swings at the player even though orcs are dummies.
    provoked: bool,
}

#[derive(Resource)]
pub struct Orcs {
    lib: Library,
    orcs: Vec<Orc>,
    idle: ClipId,
    walk: ClipId,
    run: ClipId,
    attacks: Vec<ClipId>,
    death_pose: Option<ClipId>,
    stunned: Option<ClipId>,
    stun_recover: Option<ClipId>,
    kd_loop: Option<ClipId>,
    kd_getup: Option<ClipId>,
    weapon: Option<usize>,
    hostile: bool,
}

impl Orcs {
    /// Whether the orcs have this clip (the victim half of a synchronised move).
    pub fn has_clip(&self, name: &str) -> bool {
        self.lib.find(name).is_some()
    }
}

pub struct OrcPlugin;

impl Plugin for OrcPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<IncomingBlows>()
            .init_resource::<CounterState>()
            .add_systems(Startup, spawn_prompt)
            .add_systems(Update, update_prompt);
    }
}

/// Spawn one orc per dummy and hide the placeholder capsules.
#[allow(clippy::too_many_arguments)]
pub fn load_orcs(mut commands: Commands, mut dummies: ResMut<Dummies>, mut capsules: Query<&mut Visibility, With<crate::combat::DummyBody>>) {
    match build(&mut commands, &mut dummies) {
        Ok(orcs) => {
            for mut v in &mut capsules {
                *v = Visibility::Hidden;
            }
            commands.insert_resource(orcs);
        }
        Err(err) => warn!("orcs unavailable, keeping the training dummies: {err:#}"),
    }
}

fn build(commands: &mut Commands, dummies: &mut Dummies) -> anyhow::Result<Orcs> {
    let mut vfs = Vfs::open(Vfs::locate()?)?;
    let skeleton = Skeleton::humanoid();
    let mut lib = Library::new(&skeleton)?;
    for bank in BANKS.iter().copied() {
        lib.add_bank(&vfs.read(bank)?)?;
    }
    let find = |name: &str| lib.find(name).ok_or_else(|| anyhow::anyhow!("orc clip {name} not found"));
    let idle = find("Combat_Idle2")?;
    let walk = find("Combat_Walk")?;
    let run = find("Combat_Run")?;
    let attacks: Vec<ClipId> =
        ["Bip_Cmbt_Attack_Front_1", "Bip_Cmbt_Attack_Front_3"].into_iter().filter_map(|n| lib.find(n)).collect();
    let death_pose = lib.find("Bip_DeathPose_01");

    let lowest = skeleton.bones.iter().map(|b| b.translation[1]).fold(0.0f32, f32::min);
    let mut orcs = Vec::new();
    for (i, dummy) in dummies.0.iter_mut().enumerate() {
        let entity = commands.spawn((Name::new(format!("Orc {i}")), Transform::from_translation(dummy.pos), Visibility::default())).id();
        let body = commands
            .spawn((Name::new("orc body"), Transform::from_xyz(0.0, -lowest * 0.01, 0.0), Visibility::default()))
            .id();
        commands.entity(entity).add_child(body);
        let pieces = vec![crate::character::spawn_stick(commands, &skeleton, body, Color::srgb(1.0, 0.3, 0.2))];
        let mut animator = Animator::new();
        animator.play(&lib, idle, true, 0.0, (i as f32 * 0.37).fract())?;
        dummy.animated = true;
        orcs.push(Orc {
            root: entity,
            pieces,
            animator,
            brain: Brain::Idle,
            yaw: 0.0,
            cooldown: 1.5 + i as f32,
            variety: i,
            provoked: false,
        });
    }
    info!("loaded {} orcs ({} bones)", orcs.len(), skeleton.bones.len());
    let hostile = true;
    let weapon = lib.find_bone("R_Weapon");
    let stunned = lib.find("Orc_Recoil_Stunned");
    let stun_recover = lib.find("Orc_Recoil_StunRecover");
    let kd_loop = lib.find("Orc_Recoil_KDLoop");
    let kd_getup = lib.find("Orc_Recoil_KDGetup");
    Ok(Orcs { lib, orcs, idle, walk, run, attacks, death_pose, stunned, stun_recover, kd_loop, kd_getup, weapon, hostile })
}

/// Seconds of a recoil clip before anything visibly moves: the first frame whose pose differs from frame 0.
fn lead_in(lib: &Library, clip: ClipId) -> f32 {
    let start = lib.start_ms(clip);
    let Ok(first) = lib.sample(clip, start) else { return 0.0 };
    let mut t = 33.0;
    while t < lib.duration_ms(clip).min(600.0) {
        if let Ok(now) = lib.sample(clip, start + t) {
            if crate::combat::pose_gap(&first, &now, lib.root_bone()) > 0.0008 {
                return (t - 33.0).max(0.0) / 1000.0;
            }
        }
        t += 33.0;
    }
    0.0
}

fn shortest(from: f32, to: f32) -> f32 {
    (to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// The recoil clip for a blow that came from `hit.from` onto an orc at `pos` facing `yaw`.
fn recoil_clip(lib: &Library, pos: Vec3, yaw: f32, hit: &Hit) -> Option<ClipId> {
    let facing = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let to_attacker = Vec3::new(hit.from.x - pos.x, 0.0, hit.from.z - pos.z).normalize_or_zero();
    let side = if facing.dot(to_attacker) >= 0.0 { "F" } else { "B" };
    let swing = hit.swing;
    let code = if swing == "ST" { "STR" } else { swing };
    let mut names: Vec<String> = Vec::new();
    if hit.leg {
        names.push(format!("Orc_Recoil_LegSweep_{}_0{}", if swing == "R2L" { "R2L" } else { "L2R" }, 1 + (hit.from.x * 7.0).abs() as u32 % 2));
    }
    if hit.result == HitResult::Down && !hit.kill {
        // Knocked off his feet: the knock-down recoil of the blow's swing.
        names.push(format!("Orc_Recoil_{side}_KD_{code}"));
        names.push(format!("Orc_Recoil_{side}_KD_STR"));
    }
    if hit.result == HitResult::Stun && !hit.kill && !hit.heavy {
        names.push(format!("Orc_Recoil_{side}_ToStun_{code}"));
        names.push(format!("Orc_Recoil_{side}_ToStun_STR"));
    }
    if hit.kill {
        // A knock-down that ends lying on the ground.
        names.push(format!("Bip_Recoil_Cmbt_Flurry_Ender_KD_{side}"));
    }
    if hit.heavy || hit.kill {
        names.push(format!("OrcInf_Recoil_Heavy_{swing}_{side}"));
        names.push(format!("OrcInf_Recoil_Heavy_ST_{side}"));
    }
    let reach = if hit.heavy { "200" } else { "100" };
    names.push(format!("Orc_Recoil_{side}_{swing}_{reach}cm"));
    names.push(format!("Orc_Recoil_{side}_St_{reach}cm"));
    names.push(format!("Orc_Recoil_{side}_R2L_{reach}cm"));
    names.into_iter().find_map(|n| lib.find(&n))
}

#[allow(clippy::too_many_arguments)]
pub fn update_orcs(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    options: Res<crate::shot::Options>,
    orcs: Option<ResMut<Orcs>>,
    mut dummies: ResMut<Dummies>,
    player: Option<Res<crate::player::Player>>,
    mut blows: ResMut<IncomingBlows>,
    mut counters: ResMut<CounterState>,
    mut held: Local<bool>,
    geometry: Res<crate::world::WorldGeometry>,
    mut transforms: Query<&mut Transform>,
) {
    let (Some(mut orcs), Some(player)) = (orcs, player) else { return };
    let dt = time.delta_secs().min(0.05);
    let orcs = &mut *orcs;
    let lib = &orcs.lib;
    let hostile = orcs.hostile;

    // `T` provokes the nearest standing orc.
    let down = options.pressed(&keys, &time, KeyCode::KeyT);
    if down && !*held {
        let nearest = orcs
            .orcs
            .iter()
            .enumerate()
            .filter(|(i, o)| dummies.0[*i].hp > 0.0 && matches!(o.brain, Brain::Idle | Brain::Chase))
            .min_by(|a, b| {
                let d = |i: usize| (dummies.0[i].pos - player.pos).length();
                d(a.0).total_cmp(&d(b.0))
            })
            .map(|(i, _)| i);
        if let Some(i) = nearest {
            orcs.orcs[i].provoked = true;
            orcs.orcs[i].cooldown = 0.0;
            orcs.orcs[i].brain = Brain::Chase;
        }
    }
    *held = down;

    // Only a couple of orcs press the attack at once, as the game's attack tokens do.
    let mut attacking = orcs.orcs.iter().filter(|o| matches!(o.brain, Brain::Attack { .. })).count();
    let request = counters.request.take();
    let mut prompt: Option<usize> = None;

    let others: Vec<(Vec3, bool)> = dummies.0.iter().map(|d| (d.pos, d.hp > 0.0)).collect();
    for (i, orc) in orcs.orcs.iter_mut().enumerate() {
        let Some(dummy) = dummies.0.get_mut(i) else { continue };
        orc.cooldown = (orc.cooldown - dt).max(0.0);
        let to_player = Vec3::new(player.pos.x - dummy.pos.x, 0.0, player.pos.z - dummy.pos.z);
        let dist = to_player.length();
        let want_yaw = (-to_player.x).atan2(-to_player.z);
        let ended = orc.animator.ended();
        let elapsed = orc.animator.elapsed_s(lib);
        let play = |orc: &mut Orc, clip: ClipId, looped: bool, fade: f32, phase: f32| {
            if let Err(err) = orc.animator.play(lib, clip, looped, fade, phase) {
                error_once!("orc animation failed: {err:#}");
            }
        };
        let aggressive = hostile || orc.provoked;
        dummy.aware = aggressive || !matches!(orc.brain, Brain::Idle);
        dummy.yaw = orc.yaw;

        // The player answered the prompt: play this orc's half of the counter.
        if let Some(req) = request.as_ref().filter(|r| r.orc == i) {
            let facing = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
            match req.place {
                Place::Facing => {
                    let place = if req.front { facing } else { -facing };
                    dummy.pos = player.pos + place * 1.1;
                    let back = player.pos - dummy.pos;
                    orc.yaw = (-back.x).atan2(-back.z);
                }
                Place::SameWay => {
                    dummy.pos = player.pos + facing * 0.9;
                    orc.yaw = player.yaw;
                }
            }
            dummy.pos.y = 0.0;
            orc.brain = Brain::Sync;
            orc.provoked = false;
            dummy.hit = None;
            if let Some(clip) = lib.find(&req.victim).or_else(|| lib.find(&req.victim.replace("_NoSev", ""))) {
                play(orc, clip, false, 0.05, 0.0);
            }
        }

        // A fresh hit interrupts whatever it was doing (a counter in progress is not interrupted). An orc lying on the ground
        // just takes it; the one that dies there stays down.
        if let Some(hit) = dummy.hit.take() {
            if matches!(orc.brain, Brain::Down) {
                if hit.kill {
                    orc.brain = Brain::Dead;
                }
            } else if !matches!(orc.brain, Brain::Sync) {
                orc.brain = Brain::Recoil { kill: dummy.hp <= 0.0 || hit.kill };
                if let Some(clip) = recoil_clip(lib, dummy.pos, orc.yaw, &hit) {
                    // Skip the part of the clip where nothing moves yet, so the reaction starts on the blow.
                    let skip = lead_in(lib, clip);
                    debug!("recoil {} lead-in {skip:.2}s", lib.name(clip));
                    play(orc, clip, false, 0.03, skip / (lib.duration_ms(clip) / 1000.0).max(0.1));
                }
            }
        }
        // Back from the dead: the dummy was reset.
        if matches!(orc.brain, Brain::Dead) && dummy.hp > 0.0 {
            orc.brain = Brain::Idle;
            orc.provoked = false;
            orc.yaw = 0.0;
            play(orc, orcs.idle, true, 0.0, 0.0);
        }

        match orc.brain {
            Brain::Idle => {
                if dummy.hp > 0.0 {
                    if hostile && dist < NOTICE_RANGE {
                        orc.brain = Brain::Chase;
                    } else {
                        // A dummy watches the player once he is close, unless he is crouching: then it does not notice him.
                        if !player.is_crouching() && dist < 5.5 {
                            orc.yaw += shortest(orc.yaw, want_yaw).clamp(-1.2 * dt, 1.2 * dt);
                        }
                    }
                }
            }
            Brain::Chase => {
                if !aggressive {
                    orc.brain = Brain::Idle;
                    play(orc, orcs.idle, true, 0.3, 0.0);
                } else if dist > NOTICE_RANGE * 1.4 {
                    orc.brain = Brain::Idle;
                    orc.provoked = false;
                    play(orc, orcs.idle, true, 0.3, 0.0);
                } else if dist <= ATTACK_RANGE {
                    if orc.cooldown <= 0.0 && attacking < 2 && !orcs.attacks.is_empty() {
                        orc.variety += 1;
                        let clip = orcs.attacks[orc.variety % orcs.attacks.len()];
                        orc.brain = Brain::Attack { hit_done: false };
                        attacking += 1;
                        play(orc, clip, false, 0.15, 0.0);
                    } else {
                        // Circle in place facing the player until it is its turn.
                        orc.yaw += shortest(orc.yaw, want_yaw).clamp(-4.0 * dt, 4.0 * dt);
                        play(orc, orcs.idle, true, 0.25, 0.0);
                    }
                } else {
                    orc.yaw += shortest(orc.yaw, want_yaw).clamp(-3.5 * dt, 3.5 * dt);
                    let clip = if dist > 6.0 { orcs.run } else { orcs.walk };
                    play(orc, clip, true, 0.25, 0.0);
                }
            }
            Brain::Attack { hit_done } => {
                let clip = orc.animator.current();
                let facing_window = clip.and_then(|c| lib.event_s(c, "STARTFACING").zip(lib.event_s(c, "STOPFACING")));
                if let Some((from, to)) = facing_window {
                    if elapsed >= from && elapsed <= to {
                        orc.yaw += shortest(orc.yaw, want_yaw).clamp(-5.0 * dt, 5.0 * dt);
                    }
                }
                // The blow lands on the clip's `APPLYDAMAGE` cue; some orc clips carry cues past their own end, so then
                // the instant the weapon hand is moving fastest stands in for it.
                let strike = clip
                    .map(|c| {
                        let seconds = lib.duration_ms(c) / 1000.0;
                        match lib.event_s(c, "APPLYDAMAGE") {
                            Some(t) if t < seconds - 0.1 => t,
                            _ => orcs
                                .weapon
                                .and_then(|bone| lib.fastest_s(c, bone, seconds * 0.25, seconds * 0.95))
                                .unwrap_or(seconds * 0.6),
                        }
                    })
                    .unwrap_or(1.0);
                let facing = Vec3::new(-orc.yaw.sin(), 0.0, -orc.yaw.cos());
                // The counter prompt: from the wind-up until just before the blow lands.
                if !hit_done && elapsed >= 0.15 && elapsed < strike - 0.05 && dist < 3.5 && to_player.normalize_or_zero().dot(facing) > 0.3 {
                    prompt = Some(i);
                }
                if !hit_done && elapsed >= strike {
                    orc.brain = Brain::Attack { hit_done: true };
                    if dist < ATTACK_RANGE + 0.7 && to_player.normalize_or_zero().dot(facing) > 0.5 {
                        blows.0.push(Blow { damage: ORC_DAMAGE, point: Vec3::new(player.pos.x, 1.3, player.pos.z) });
                    }
                }
                if ended {
                    orc.cooldown = 1.6 + (orc.variety % 3) as f32 * 0.5;
                    orc.provoked = false;
                    orc.brain = if aggressive && hostile { Brain::Chase } else { Brain::Idle };
                    play(orc, orcs.idle, true, 0.25, 0.0);
                }
            }
            Brain::Recoil { kill } => {
                if ended {
                    if kill {
                        // The knock-down ends on the ground; the last frame holds.
                        orc.brain = Brain::Dead;
                    } else if dummy.down > 0.0 && orcs.kd_loop.is_some() {
                        orc.brain = Brain::Down;
                        play(orc, orcs.kd_loop.unwrap(), true, 0.1, 0.0);
                    } else if (dummy.stun > 0.0 || dummy.quick > 0.0) && orcs.stunned.is_some() {
                        orc.brain = Brain::Stunned;
                        play(orc, orcs.stunned.unwrap(), true, 0.1, 0.0);
                    } else {
                        orc.brain = if hostile { Brain::Chase } else { Brain::Idle };
                        play(orc, orcs.idle, true, 0.2, 0.0);
                    }
                }
            }
            Brain::Stunned => {
                if dummy.down > 0.0 && orcs.kd_loop.is_some() {
                    orc.brain = Brain::Down;
                    play(orc, orcs.kd_loop.unwrap(), true, 0.1, 0.0);
                } else if dummy.stun <= 0.0 && dummy.quick <= 0.0 {
                    orc.brain = Brain::Recover;
                    play(orc, orcs.stun_recover.unwrap_or(orcs.idle), false, 0.15, 0.0);
                }
            }
            Brain::Down => {
                if dummy.down <= 0.0 {
                    orc.brain = Brain::Recover;
                    play(orc, orcs.kd_getup.unwrap_or(orcs.idle), false, 0.15, 0.0);
                }
            }
            Brain::Recover => {
                if ended {
                    orc.brain = if hostile { Brain::Chase } else { Brain::Idle };
                    play(orc, orcs.idle, true, 0.2, 0.0);
                }
            }
            Brain::Sync => {
                if ended {
                    if dummy.hp <= 0.0 {
                        orc.brain = Brain::Dead;
                    } else if dummy.down > 0.0 && orcs.kd_loop.is_some() {
                        orc.brain = Brain::Down;
                        play(orc, orcs.kd_loop.unwrap(), true, 0.1, 0.0);
                    } else if dummy.stun > 0.0 && orcs.stunned.is_some() {
                        orc.brain = Brain::Stunned;
                        play(orc, orcs.stunned.unwrap(), true, 0.1, 0.0);
                    } else {
                        orc.brain = Brain::Idle;
                        play(orc, orcs.idle, true, 0.2, 0.0);
                    }
                }
            }
            Brain::Dead => {}
        }

        // Animate; the clip's root travel moves the orc.
        let step = match orc.animator.update(lib, dt) {
            Ok(step) => step,
            Err(err) => {
                error_once!("orc animation failed: {err:#}");
                continue;
            }
        };
        // In a counter the orc stays where it was placed: its clip's travel was authored against the player's.
        let synced = matches!(orc.brain, Brain::Sync);
        if !synced {
            orc.yaw -= step.yaw;
        }
        let local = if synced { Vec3::ZERO } else { to_bevy_vec([step.travel[0], 0.0, step.travel[2]]) };
        let world = Quat::from_rotation_y(orc.yaw) * local;
        dummy.pos.x = (dummy.pos.x + world.x).clamp(-49.0, 49.0);
        dummy.pos.z = (dummy.pos.z + world.z).clamp(-49.0, 49.0);
        dummy.pos.y = 0.0;
        if !synced {
            dummy.pos = geometry.push_out(dummy.pos, crate::world::BODY_RADIUS);
        }
        // Keep apart from the others and from the player (soft body radius ~0.55 m each), except in a counter.
        if dummy.hp > 0.0 && !matches!(orc.brain, Brain::Sync) {
            let mut push = Vec3::ZERO;
            for (j, &(pos, alive)) in others.iter().enumerate() {
                if j != i && alive {
                    let away = Vec3::new(dummy.pos.x - pos.x, 0.0, dummy.pos.z - pos.z);
                    let d = away.length();
                    if d < 1.1 && d > 1e-3 {
                        push += away / d * (1.1 - d);
                    }
                }
            }
            let away = Vec3::new(dummy.pos.x - player.pos.x, 0.0, dummy.pos.z - player.pos.z);
            let d = away.length();
            if d < 1.0 && d > 1e-3 {
                push += away / d * (1.0 - d);
            }
            dummy.pos += push * (10.0 * dt).min(1.0);
        }
        if let Ok(mut t) = transforms.get_mut(orc.root) {
            t.translation = dummy.pos;
            t.rotation = Quat::from_rotation_y(orc.yaw);
        }
        for joints in &orc.pieces {
            for (joint, bone) in joints.iter().zip(&step.pose) {
                if let Ok(mut t) = transforms.get_mut(*joint) {
                    t.translation = to_bevy_vec(bone.t);
                    t.rotation = to_bevy_quat(bone.r);
                }
            }
        }
    }
    counters.prompt = prompt;
}

#[derive(Component)]
struct CounterPrompt;

#[derive(Component)]
struct FinisherPrompt;

/// A key prompt as the game shows it: the key in a box.
fn key_box(commands: &mut Commands, key: &str, tint: Color) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                padding: UiRect::axes(Val::Px(9.0), Val::Px(2.0)),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.05, 0.72)),
            BorderColor::all(tint),
            Visibility::Hidden,
        ))
        .with_children(|b| {
            b.spawn((Text::new(key), TextFont { font_size: bevy::text::FontSize::Px(22.0), ..default() }, TextColor(tint)));
        })
        .id()
}

#[derive(Component)]
struct CounterRing;

/// The counter prompt as the game shows it over the attacker: a mouse with its right button lit, inside a ring that pulses in.
fn spawn_counter_icon(commands: &mut Commands) {
    let white = Color::srgb(0.96, 0.97, 1.0);
    commands
        .spawn((
            Node { position_type: PositionType::Absolute, width: Val::Px(34.0), height: Val::Px(48.0), ..default() },
            Visibility::Hidden,
            CounterPrompt,
        ))
        .with_children(|b| {
            b.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(-17.0),
                    top: Val::Px(-10.0),
                    width: Val::Px(68.0),
                    height: Val::Px(68.0),
                    border: UiRect::all(Val::Px(2.0)),
                    border_radius: BorderRadius::MAX,
                    ..default()
                },
                BorderColor::all(white),
                CounterRing,
            ));
            // The mouse body.
            b.spawn((
                Node { position_type: PositionType::Absolute, width: Val::Px(34.0), height: Val::Px(48.0), border: UiRect::all(Val::Px(2.5)), border_radius: BorderRadius::all(Val::Px(17.0)), ..default() },
                BackgroundColor(Color::srgba(0.02, 0.03, 0.05, 0.7)),
                BorderColor::all(white),
            ));
            // The right button, lit.
            b.spawn((
                Node { position_type: PositionType::Absolute, left: Val::Px(17.0), top: Val::Px(2.5), width: Val::Px(14.5), height: Val::Px(20.0), border_radius: BorderRadius::new(Val::ZERO, Val::Px(14.0), Val::ZERO, Val::ZERO), ..default() },
                BackgroundColor(white),
            ));
            // The divide between the buttons and the rest of the body.
            b.spawn((Node { position_type: PositionType::Absolute, left: Val::Px(2.5), top: Val::Px(22.0), width: Val::Px(29.0), height: Val::Px(2.0), ..default() }, BackgroundColor(white)));
            b.spawn((Node { position_type: PositionType::Absolute, left: Val::Px(16.0), top: Val::Px(2.5), width: Val::Px(2.0), height: Val::Px(20.0), ..default() }, BackgroundColor(white)));
        });
}

fn spawn_prompt(mut commands: Commands) {
    spawn_counter_icon(&mut commands);
    let execute = key_box(&mut commands, "F", Color::srgb(0.95, 0.97, 1.0));
    commands.entity(execute).insert(FinisherPrompt);
}

/// The prompt floats above the orc whose blow can be countered.
fn update_prompt(
    counters: Res<CounterState>,
    dummies: Res<Dummies>,
    cameras: Query<(&Camera, &GlobalTransform), With<crate::camera::FollowCamera>>,
    mut prompt: Query<(&mut Node, &mut Visibility), With<CounterPrompt>>,
    mut finisher: Query<(&mut Node, &mut Visibility), (With<FinisherPrompt>, Without<CounterPrompt>)>,
    player: Option<Res<crate::player::Player>>,
    time: Res<Time>,
    mut ring: Query<&mut BorderColor, With<CounterRing>>,
) {
    let charged = player.as_ref().is_some_and(|p| p.is_charged());
    let near = player.as_ref().map_or(Vec3::ZERO, |p| p.pos);
    if let (Ok((camera, camera_transform)), Ok((mut node, mut visibility))) = (cameras.single(), finisher.single_mut()) {
        // The Hit Streak is charged: F executes the orc in reach.
        match dummies.0.iter().find(|d| charged && d.hp > 0.0 && (d.pos - near).length() < 4.0) {
            Some(d) => match camera.world_to_viewport(camera_transform, d.pos + Vec3::Y * 2.3) {
                Ok(screen) => {
                    node.left = Val::Px(screen.x - 17.0);
                    node.top = Val::Px(screen.y - 16.0);
                    *visibility = Visibility::Inherited;
                }
                Err(_) => *visibility = Visibility::Hidden,
            },
            None => *visibility = Visibility::Hidden,
        }
    }
    let (Ok((camera, camera_transform)), Ok((mut node, mut visibility))) = (cameras.single(), prompt.single_mut()) else {
        return;
    };
    match counters.prompt.and_then(|i| dummies.0.get(i)) {
        Some(d) => match camera.world_to_viewport(camera_transform, d.pos + Vec3::Y * 2.3) {
            Ok(screen) => {
                node.left = Val::Px(screen.x - 17.0);
                node.top = Val::Px(screen.y - 24.0);
                *visibility = Visibility::Inherited;
            }
            Err(_) => *visibility = Visibility::Hidden,
        },
        None => *visibility = Visibility::Hidden,
    }
    // The ring breathes so the prompt catches the eye.
    if let Ok(mut ring) = ring.single_mut() {
        let t = (time.elapsed_secs() * 3.2).fract();
        *ring = BorderColor::all(Color::srgba(0.96, 0.97, 1.0, 1.0 - t * 0.75));
    }
}
