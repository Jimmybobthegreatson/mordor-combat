//! Freeflow combat data: Talion's attacks as the game's database defines them, how one is chosen for a
//! target, and training dummies to hit.
//!
//! The game keeps attacks in *pools* (`PC_PoolAttacks_HC_<tier>_D<n>`, see [`som_formats::combatdb`]). The
//! tier is chosen by the hit counter - the combo count - so a fight escalates from quick light cuts
//! (0-3 hits) through longer ones (4-15, 16-29) to heavy strikes (30+). Each attack starts from one foot
//! stance and ends in another, so a chain stays continuous: an attack that ends in `RR` is followed by one
//! that starts in `RR`.

use bevy::prelude::*;
use som_formats::animator::{ClipId, Library, Skin};

/// Every bank the combat needs, after the locomotion banks.
pub fn bank_paths() -> Vec<String> {
    let mut paths: Vec<String> = crate::player::BANKS.iter().map(|s| s.to_string()).collect();
    paths.extend(som_formats::banks::player_combat());
    paths
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Front,
    Left,
    Right,
    Back,
}

impl Dir {
    /// Angle (radians, positive = left) at which the clip expects its target.
    pub fn side(self) -> f32 {
        match self {
            Dir::Front => 0.0,
            Dir::Left => std::f32::consts::FRAC_PI_2,
            Dir::Right => -std::f32::consts::FRAC_PI_2,
            Dir::Back => std::f32::consts::PI,
        }
    }
}

pub const DIRS: [Dir; 4] = [Dir::Front, Dir::Left, Dir::Right, Dir::Back];

#[derive(Clone, Debug)]
pub struct Attack {
    /// The game's `Combat/Attack` record name (what `Moves::applied` is keyed by).
    pub name: String,
    pub clip: ClipId,
    /// Hit counter range (inclusive) this attack is available in.
    pub hits: (u32, u32),
    /// Hit tier 1 (`Lowest`) to 4 (`High`), from the counter range.
    pub tier: u8,
    /// Side of the character the clip expects its target on.
    pub dir: Dir,
    /// Sides the game allows the target to be on.
    pub allowed: Vec<Dir>,
    pub start: String,
    pub end: String,
    /// Heavy strikes hit harder.
    pub heavy: bool,
    /// Swing direction code for the victim's reaction: `R2L`, `L2R`, `U2D`, `D2U`, `ST`.
    pub swing: &'static str,
    /// Target distance range (cm) in which the game allows this attack.
    pub range_cm: (f32, f32),
    /// How far the root travels over the whole clip (m).
    pub reach: f32,
    /// How far it has travelled by the moment the blow lands (m): what must cover the gap to the target.
    pub strike_reach: f32,
}

/// Hit tier (1..=4) for a hit counter value, as the pools gate it: 0-3, 4-15, 16-29, 30+.
pub fn tier_of(hits: u32) -> u8 {
    match hits {
        0..=3 => 1,
        4..=15 => 2,
        16..=29 => 3,
        _ => 4,
    }
}

#[derive(Default)]
pub struct Catalog {
    pub attacks: Vec<Attack>,
    /// Flurry hits on a stunned orc (`Elf_Cmbt_FlurryStand_*`).
    pub flurry: Vec<Attack>,
    /// The game's move nodes, for choosing between a counter, a flurry, the finisher and the freeflow chain.
    pub moves: som_formats::combatnodes::Moves,
    /// Gap closers: (clip, reach in m).
    pub get_there: Vec<(ClipId, f32)>,
    /// The evade roll.
    pub roll: Option<ClipId>,
    /// Drawing and stowing the sword (the game's `PL_Sword_UnSheathe_1` / `PL_Sword_Sheathe_1`).
    pub unsheathe: Option<ClipId>,
    pub sheathe: Option<ClipId>,
    /// Hurt reactions, light to knock-back.
    pub recoils: Vec<(ClipId, &'static str)>,
}

/// How far (m) a clip's root has travelled by its blow.
pub fn strike_reach_of(lib: &Library, clip: ClipId) -> f32 {
    let t = lib.event_s(clip, "APPLYDAMAGE").unwrap_or(lib.duration_ms(clip) / 2000.0);
    lib.sample(clip, lib.start_ms(clip) + t * 1000.0)
        .map(|skin| {
            let r = skin[lib.root_bone()].t;
            r[0].hypot(r[2]) / 100.0
        })
        .unwrap_or(0.0)
}

impl Catalog {
    pub fn build(lib: &Library, attacks: &[som_formats::combatdb::PcAttack], moves: &som_formats::combatnodes::Moves) -> Self {
        let mut catalog = Catalog::default();
        catalog.moves = moves.clone();
        // The player's base pools; alternates (`Doubled`, `Tripled`, `FLM`) belong to other situations.
        let plain = |pool: &str| pool.rsplit('_').next().is_some_and(|d| d.len() == 2 && d.starts_with('D'));
        for a in attacks.iter().filter(|a| plain(&a.pool)) {
            let Some(clip) = lib.find(&a.clip) else { continue };
            let Ok((travel, _)) = lib.root_total(clip) else { continue };
            let parts: Vec<&str> = a.name.split('_').collect();
            let dir = match parts.get(5).copied() {
                Some("F") => Dir::Front,
                Some("L") => Dir::Left,
                Some("R") => Dir::Right,
                Some("B") => Dir::Back,
                _ => Dir::Front,
            };
            let mut allowed: Vec<Dir> =
                [("Forward", Dir::Front), ("Left", Dir::Left), ("Right", Dir::Right), ("Back", Dir::Back)]
                    .into_iter()
                    .filter(|(word, _)| a.allowed.contains(word))
                    .map(|(_, d)| d)
                    .collect();
            if allowed.is_empty() {
                allowed.push(dir);
            }
            catalog.attacks.push(Attack {
                name: a.name.clone(),
                clip,
                hits: a.hits,
                tier: tier_of(a.hits.0),
                dir,
                allowed,
                start: a.start.clone(),
                end: a.end.clone(),
                heavy: a.kind.starts_with("Heavy"),
                swing: match a.kind.split('_').nth(1).unwrap_or("") {
                    "RightToLeft" => "R2L",
                    "LeftToRight" => "L2R",
                    "UpToDown" => "U2D",
                    "DownToUp" => "D2U",
                    _ => "ST",
                },
                range_cm: a.range_cm,
                reach: travel[0].hypot(travel[2]) / 100.0,
                strike_reach: {
                    // Root pose at the blow (relative to the clip start): the character must have covered the gap by then.
                    let t = lib.event_s(clip, "APPLYDAMAGE").unwrap_or(lib.duration_ms(clip) / 2000.0);
                    lib.sample(clip, lib.start_ms(clip) + t * 1000.0)
                        .map(|skin| {
                            let r = skin[lib.root_bone()].t;
                            r[0].hypot(r[2]) / 100.0
                        })
                        .unwrap_or(0.0)
                },
            });
        }
        let first = crate::player::BANKS.len();
        for bank in first..lib.bank_count() {
            for (clip, name) in lib.clips(bank) {
                if name.eq_ignore_ascii_case("bind") {
                    continue;
                }
                match name {
                    "PL_Sword_UnSheathe_1" => catalog.unsheathe = Some(clip),
                    "PL_Sword_Sheathe_1" => catalog.sheathe = Some(clip),
                    _ => {}
                }
                let Ok((travel, _)) = lib.root_total(clip) else { continue };
                let reach = travel[0].hypot(travel[2]) / 100.0;
                let parts: Vec<&str> = name.split('_').collect();
                if name.starts_with("Elf_Cmbt_FlurryStand_") && !name.contains("INTO") {
                    let swing = if name.contains("L2R") { "L2R" } else { "R2L" };
                    catalog.flurry.push(Attack {
                        name: name.to_string(),
                        clip,
                        hits: (0, u32::MAX),
                        tier: 1,
                        dir: Dir::Front,
                        allowed: vec![Dir::Front],
                        start: String::new(),
                        end: String::new(),
                        heavy: false,
                        swing,
                        range_cm: (0.0, 10_000.0),
                        reach,
                        strike_reach: strike_reach_of(lib, clip),
                    });
                } else if name.starts_with("Elf_GT_Run") && parts.get(4) == Some(&"F") {
                    catalog.get_there.push((clip, reach));
                } else if name == "Elf_Dash_Chain_A" {
                    catalog.roll = Some(clip);
                } else if name.starts_with("Elf_Recoil_") {
                    let weight = match parts.get(2) {
                        Some(&"Light") => "light",
                        Some(&"Medium") => "medium",
                        Some(&"Heavy") => "heavy",
                        _ => continue, // knock-backs / knock-downs need the ground game
                    };
                    catalog.recoils.push((clip, weight));
                }
            }
        }
        catalog
    }

    /// The attack to use at `level` for a target `distance` metres away on `side` (radians from the
    /// character's heading, positive = left), continuing from foot stance `stance`. `variety` rotates
    /// through equally good candidates.
    /// The next freeflow attack, chosen as the game's `PC_PoolAttacks` node does: every attack of the pool has the same
    /// `AttackExplicitValue`, so the candidates are simply those whose predicates hold - hit-counter tier, a side the target is
    /// on, the distance class that covers it, and a start stance equal to the stance he is in - and one of them is taken at
    /// random, passing over the ones just used (the pool's `VariationType`, `Pool_Basic_Attacks`).
    pub fn choose(&self, lib: &Library, hits: u32, side: f32, distance: Option<f32>, stance: &str, recent: &[String], seed: u32) -> Option<&Attack> {
        let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        let dir = DIRS
            .into_iter()
            .min_by(|a, b| wrap(side - a.side()).abs().total_cmp(&wrap(side - b.side()).abs()))
            .unwrap_or(Dir::Front);
        let cm = distance.map_or(120.0, |d| d * 100.0);
        let pool = |same_stance: bool, in_range: bool, same_side: bool| {
            self.attacks
                .iter()
                .filter(|a| {
                    a.hits.0 <= hits
                        && hits <= a.hits.1
                        && (!same_side || a.allowed.contains(&dir))
                        && (!in_range || (a.range_cm.0 <= cm && cm < a.range_cm.1))
                        && (!same_stance || a.start.is_empty() || a.start == stance)
                })
                .collect::<Vec<_>>()
        };
        // The data covers every stance / distance / side; the relaxed passes only catch the clips missing from this install.
        let mut candidates = pool(true, true, true);
        for (stance, range, side) in [(false, true, true), (true, false, true), (false, false, true), (false, false, false)] {
            if !candidates.is_empty() {
                break;
            }
            candidates = pool(stance, range, side);
        }
        if candidates.is_empty() {
            return None;
        }
        // Variation: not one of the last few, as long as something else is left.
        let window = (candidates.len() / 2).min(4);
        let fresh: Vec<&Attack> = candidates
            .iter()
            .copied()
            .filter(|a| !recent.iter().rev().take(window).any(|n| n == lib.name(a.clip)))
            .collect();
        let from = if fresh.is_empty() { candidates } else { fresh };
        let roll = seed.wrapping_mul(2_654_435_761).rotate_left(13) ^ hits.wrapping_mul(40_503);
        from.get(roll as usize % from.len()).copied()
    }
}

/// What the move predicates are evaluated against.
#[derive(Clone)]
pub struct SelectCtx<'a> {
    /// Weapon carried (`PC_Sword`).
    pub item: &'a str,
    pub hits: u32,
    /// Inventory items the player carries (`CSP_HeadExp`: the gore setting that picks the killing finishers).
    pub items: &'a [&'a str],
    pub target: Option<TargetCtx>,
    /// The finisher key was pressed: only the flurry / ender nodes are candidates; otherwise they are not.
    pub finisher: bool,
    /// When not empty, only nodes whose name starts with this are candidates (a stunned orc takes the knock-down pool).
    pub only: &'a str,
}

#[derive(Clone)]
pub struct TargetCtx {
    pub distance_cm: f32,
    /// Where the target is relative to the player.
    pub side: Dir,
    pub quick_stunned: bool,
    /// In the plain `Stunned` state (what freeflow hits apply).
    pub stunned: bool,
    pub knocked_down: bool,
    /// The target's current attack has its parry window open (between `BPW` and `EPW`).
    pub parry_open: bool,
    /// Remaining health in the game's units.
    pub health: f32,
}

pub fn side_word(d: Dir) -> &'static str {
    match d {
        Dir::Front => "Forward",
        Dir::Left => "Left",
        Dir::Right => "Right",
        Dir::Back => "Back",
    }
}

/// Choosing a move from the game's nodes.
pub trait Select {
    fn select(&self, input: &str, ctx: &SelectCtx<'_>, variety: usize) -> Option<&som_formats::combatnodes::AttackDef>;
}

/// Whether `a`'s predicate holds in `ctx` (the checks the engine's `EvaluteTarget`/`EvaluteSelf` make that we can see).
fn passes(a: &som_formats::combatnodes::AttackDef, ctx: &SelectCtx<'_>) -> bool {
    if !(a.item.is_empty() || a.item == ctx.item || a.item == "PC_SwordOrDagger") {
        return false;
    }
    if !(a.hits.0 <= ctx.hits && ctx.hits <= a.hits.1) {
        return false;
    }
    // Player inventory items, and the other character types' moves (the orcs here are plain biped orcs).
    if (!a.needs_item.is_empty() && !ctx.items.contains(&a.needs_item.as_str()))
        || (!a.forbids_item.is_empty() && ctx.items.contains(&a.forbids_item.as_str()))
        || !a.target_needs.is_empty()
        || a.name.contains("Goblin")
        || a.name.contains("Bzrk")
        || a.name.starts_with("DLC")
    {
        return false;
    }
    let Some(t) = &ctx.target else { return !a.require_target && a.parry.is_empty() && a.target_states.is_empty() };
    let sides = if a.allowed.is_empty() { &a.attacker_sides } else { &a.allowed };
    if !sides.is_empty() && !sides.contains(side_word(t.side)) {
        return false;
    }
    if !(a.range_cm.0 <= t.distance_cm && t.distance_cm < a.range_cm.1) {
        return false;
    }
    let states_ok = match a.target_states.as_str() {
        "" => true,
        // The flurry / ender lists name `Quick_Stunned`; freeflow hits only apply plain `Stunned` (see NOTES.md), so a stunned orc counts.
        "Quick_Stunned" => t.quick_stunned || t.stunned,
        "NormalOrStunned_NotQuickStnd" => !t.quick_stunned && !t.knocked_down,
        "NormalOrStunned" => !t.knocked_down,
        "NormalOnly" => !t.quick_stunned && !t.stunned && !t.knocked_down,
        "StunnedOnly" => t.stunned && !t.quick_stunned && !t.knocked_down,
        "KnockedDown" | "KnockedDown_All" => t.knocked_down,
        _ => true,
    };
    let parry_ok = match a.parry.as_str() {
        "" => true,
        "PC_ParrySuccess" => t.parry_open,
        _ => false,
    };
    states_ok && parry_ok
}


impl Select for som_formats::combatnodes::Moves {
    /// The move the game would start for `input`: nodes of that input in priority order (lowest number first); the first
    /// priority level with a valid candidate wins, and among its candidates the highest `AttackExplicitValue`, ties
    /// rotating with `variety`. Nodes that are not ground melee, and sprint counters, are not considered.
    fn select(&self, input: &str, ctx: &SelectCtx<'_>, variety: usize) -> Option<&som_formats::combatnodes::AttackDef> {
        let mut levels: Vec<u32> =
            self.nodes.iter().filter(|n| n.input == input && (!n.name.starts_with("PC_InstaKill") || n.name == "PC_InstaKill_KBM")).map(|n| n.priority).collect();
        levels.sort_unstable();
        levels.dedup();
        for level in levels {
            let mut best: Vec<&som_formats::combatnodes::AttackDef> = Vec::new();
            for node in self.nodes.iter().filter(|n| n.input == input && n.priority == level) {
                // Counters: the basic pool and the stun / knock-down / knock-back / kill outcomes; the rest are named/special
                // enemies, companions and upgrades we do not have.
                if matches!(node.name.as_str(), "PC_Counter" | "PC_Counter_Companion" | "PC_Counter_Inf_Upgrade_Fury") {
                    continue;
                }
                if !ctx.only.is_empty() && !node.name.starts_with(ctx.only) {
                    continue;
                }
                // The finisher key chooses the flurry / ender family, every other input never does.
                if node.name.starts_with("PC_Flurry_Standing") != ctx.finisher {
                    continue;
                }
                for &i in &node.attacks {
                    let a = &self.attacks[i];
                    if a.name.starts_with("PC_Spr_") || a.name.contains("Ledge") || !passes(a, ctx) {
                        continue;
                    }
                    match best.first().map(|b| b.explicit) {
                        Some(v) if a.explicit < v => {}
                        Some(v) if a.explicit == v => best.push(a),
                        _ => {
                            best.clear();
                            best.push(a);
                        }
                    }
                }
            }
            if !best.is_empty() {
                // Flurry hits go in order: the n-th node first.
                best.sort_by(|a, b| a.node.cmp(&b.node));
                return best.get(variety % best.len()).copied();
            }
        }
        None
    }
}

/// How different two poses look: mean rotation difference over the bones (0 = identical, 1 = opposite).
pub fn pose_gap(a: &Skin, b: &Skin, root: usize) -> f32 {
    let mut sum = 0.0;
    let mut n = 0.0;
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if i == root {
            continue;
        }
        let dot: f32 = x.r.iter().zip(&y.r).map(|(p, q)| p * q).sum();
        sum += 1.0 - dot.abs().min(1.0);
        n += 1.0;
    }
    if n > 0.0 { sum / n } else { 0.0 }
}

/// Distance (m) the character should end up from the target's centre.
pub const STAND_OFF: f32 = 1.2;

const DUMMY_HP: f32 = 100.0;

/// The training orc's health; `SOM_ORC_HP` overrides it (a tough one lets the Hit Streak build on a single orc).
pub fn dummy_hp() -> f32 {
    std::env::var("SOM_ORC_HP").ok().and_then(|v| v.parse().ok()).unwrap_or(DUMMY_HP)
}
/// Short, so the Hit Streak can carry over to the respawned orc.
const RESPAWN_AFTER: f32 = 2.5;

/// A training dummy.
pub struct Dummy {
    pub pos: Vec3,
    pub hits: u32,
    pub hp: f32,
    /// Seconds since it fell, while dead.
    pub dead_for: f32,
    pub flash: f32,
    /// Knock-back velocity (m/s) that decays.
    pub push: Vec3,
    /// An animated orc drives this dummy; its clips carry the movement.
    pub animated: bool,
    /// The last blow, for the orc to react to.
    pub hit: Option<crate::orc::Hit>,
    /// Seconds left in the plain `Stunned` state, `Quick_Stunned` and `KnockDown` (the game's `AppliedState` durations).
    pub stun: f32,
    pub quick: f32,
    pub down: f32,
    /// The orc has noticed the player (provoked or hostile); an unaware one can be stealth-killed.
    pub aware: bool,
    /// Heading of the orc (Bevy yaw), kept up to date by the orc system.
    pub yaw: f32,
}

#[derive(Resource, Default)]
pub struct Dummies(pub Vec<Dummy>, pub Option<usize>);

pub struct DummyPlugin;

impl Plugin for DummyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Dummies>()
            .init_resource::<HitStop>()
            .init_resource::<ImpactFx>()
            .add_systems(Update, (apply_hit_stop, spawn_sparks, update_sparks))
            .add_systems(Startup, (spawn_dummies, spawn_lock_marker, crate::orc::load_orcs).chain())
            .add_systems(Update, (update_dummies, crate::orc::update_orcs, update_lock_marker).chain().after(crate::player::drive_player));
    }
}

/// One training orc (a grunt) for now.
const DUMMY_SPOTS: [Vec3; 1] = [Vec3::new(0.0, 0.0, -5.0)];

#[derive(Component)]
pub struct DummyBody(usize);

fn spawn_dummies(
    mut commands: Commands,
    mut dummies: ResMut<Dummies>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh = meshes.add(Capsule3d::new(0.32, 1.1));
    for (i, spot) in DUMMY_SPOTS.into_iter().enumerate() {
        let material = materials.add(StandardMaterial {
            base_color: Color::srgb(0.45, 0.3, 0.22),
            perceptual_roughness: 0.9,
            ..default()
        });
        let entity = commands
            .spawn((
                Name::new(format!("Dummy {i}")),
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material),
                Transform::from_translation(spot + Vec3::Y * 0.87),
                DummyBody(i),
            ))
            .id();
        let _ = entity;
        dummies.0.push(Dummy { pos: spot, hits: 0, hp: dummy_hp(), dead_for: 0.0, flash: 0.0, push: Vec3::ZERO, animated: false, hit: None, stun: 0.0, quick: 0.0, down: 0.0, aware: false, yaw: 0.0 });
    }
}

fn update_dummies(
    time: Res<Time>,
    mut dummies: ResMut<Dummies>,
    mut bodies: Query<(&DummyBody, &mut Transform, &MeshMaterial3d<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let dt = time.delta_secs();
    for (i, dummy) in dummies.0.iter_mut().enumerate() {
        if dummy.hp <= 0.0 {
            dummy.dead_for += dt;
            if dummy.dead_for > RESPAWN_AFTER {
                dummy.hp = dummy_hp();
                dummy.stun = 0.0;
                dummy.quick = 0.0;
                dummy.down = 0.0;
                dummy.dead_for = 0.0;
                dummy.pos = DUMMY_SPOTS[i % DUMMY_SPOTS.len()];
                dummy.push = Vec3::ZERO;
            }
        }
        dummy.flash = (dummy.flash - dt * 4.0).max(0.0);
        dummy.stun = (dummy.stun - dt).max(0.0);
        dummy.quick = (dummy.quick - dt).max(0.0);
        dummy.down = (dummy.down - dt).max(0.0);
        let push = dummy.push;
        dummy.pos += push * dt;
        dummy.pos.y = 0.0;
        dummy.push *= (1.0 - 6.0 * dt).max(0.0);
    }
    for (body, mut transform, material) in &mut bodies {
        let dummy = &dummies.0[body.0];
        transform.translation = dummy.pos + Vec3::Y * 0.87;
        // Lean away from the last shove.
        let lean = (dummy.push.length() * 0.05).min(0.5);
        // A dead dummy topples backwards, away from the last blow.
        let fall = if dummy.hp <= 0.0 { (dummy.dead_for * 3.0).min(1.0) * std::f32::consts::FRAC_PI_2 } else { 0.0 };
        let away = dummy.push.normalize_or_zero();
        let tilt_axis = Vec3::new(away.z, 0.0, -away.x);
        transform.rotation = Quat::from_rotation_z(lean * dummy.push.x.signum())
            * Quat::from_rotation_x(-lean * dummy.push.z.signum())
            * Quat::from_axis_angle(tilt_axis.normalize_or_zero(), fall);
        if fall > 0.0 {
            transform.translation.y = 0.35 + 0.52 * (1.0 - fall / std::f32::consts::FRAC_PI_2);
        }
        if let Some(mut m) = materials.get_mut(&material.0) {
            let f = dummy.flash;
            m.emissive = LinearRgba::rgb(f * 3.0, f * 0.9, f * 0.2);
        }
    }
}

impl Dummies {
    /// The dummy nearest to `pos` within `reach` metres whose bearing is within `half_cone` of `heading`
    /// (a ground direction); returns its index.
    pub fn nearest(&self, pos: Vec3, heading: Vec3, reach: f32, half_cone: f32) -> Option<usize> {
        self.0
            .iter()
            .enumerate()
            .filter(|(_, d)| d.hp > 0.0)
            .filter_map(|(i, d)| {
                let to = Vec3::new(d.pos.x - pos.x, 0.0, d.pos.z - pos.z);
                let dist = to.length();
                (dist <= reach && dist > 1e-3 && to.normalize().dot(heading) >= half_cone.cos()).then_some((i, dist))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Soft lock-on, as freeflow does it: the target is whoever the stick (or, without input, the character)
    /// points at, kept until it dies or gets far away, and only swapped when the stick clearly asks for another.
    pub fn update_lock(&mut self, from: Vec3, facing: Vec3, wanted: Vec3, engaged: bool) {
        if !engaged {
            self.1 = None;
            return;
        }
        let aim = if wanted != Vec3::ZERO { wanted } else { facing };
        let angle_to = |d: &Dummy| {
            let to = Vec3::new(d.pos.x - from.x, 0.0, d.pos.z - from.z);
            to.normalize_or_zero().dot(aim).clamp(-1.0, 1.0).acos()
        };
        let dist_to = |d: &Dummy| Vec2::new(d.pos.x - from.x, d.pos.z - from.z).length();
        if self.1.is_some_and(|i| self.0[i].hp <= 0.0 || dist_to(&self.0[i]) > 14.0) {
            self.1 = None;
        }
        let limit = if wanted != Vec3::ZERO { 70f32.to_radians() } else { 110f32.to_radians() };
        let best = self
            .0
            .iter()
            .enumerate()
            .filter(|(_, d)| d.hp > 0.0 && dist_to(d) < 12.0 && angle_to(d) < limit)
            .min_by(|a, b| (angle_to(a.1) + 0.06 * dist_to(a.1)).total_cmp(&(angle_to(b.1) + 0.06 * dist_to(b.1))))
            .map(|(i, _)| i);
        match (self.1, best) {
            (None, best) => self.1 = best,
            (Some(cur), Some(best)) if best != cur && wanted != Vec3::ZERO => {
                if angle_to(&self.0[best]) < 30f32.to_radians() && angle_to(&self.0[cur]) > 50f32.to_radians() {
                    self.1 = Some(best);
                }
            }
            _ => {}
        }
    }

    /// A blow lands: damage, and the states the attack's categories apply (`applied`, with the hit counter `hits` picking the
    /// tier): `Stunned`, `Quick_Stunned` or `KnockDown`, each with the game's chance and duration.
    #[allow(clippy::too_many_arguments)]
    pub fn strike(
        &mut self,
        index: usize,
        from: Vec3,
        power: f32,
        damage: f32,
        swing: &'static str,
        heavy: bool,
        lethal: bool,
        applied: &[som_formats::combatnodes::Applied],
        hits: u32,
        leg: bool,
    ) {
        let d = &mut self.0[index];
        d.hits += 1;
        // A flurry hit never kills: the ender does.
        d.hp = (d.hp - damage).max(if lethal { 0.0 } else { d.hp.min(3.0) });
        d.flash = 1.0;
        let mut result = crate::orc::HitResult::Plain;
        for (n, a) in applied.iter().filter(|a| a.hits.0 <= hits && hits <= a.hits.1).enumerate() {
            let roll = (d.hits.wrapping_add(n as u32 * 7).wrapping_mul(2_654_435_761) >> 7) % 100;
            if d.hp <= 0.0 || (roll as f32) >= a.chance * 100.0 {
                continue;
            }
            // The duration is a range: somewhere between its ends.
            let t = ((d.hits.wrapping_mul(40_503) >> 3) % 100) as f32 / 100.0;
            let seconds = a.duration + (a.duration_max - a.duration).max(0.0) * t;
            match a.state.as_str() {
                "KnockDown" => {
                    d.down = d.down.max(seconds);
                    result = crate::orc::HitResult::Down;
                }
                "Quick_Stunned" => {
                    d.quick = d.quick.max(seconds);
                    if result == crate::orc::HitResult::Plain {
                        result = crate::orc::HitResult::Stun;
                    }
                }
                _ => {
                    d.stun = d.stun.max(seconds);
                    if result == crate::orc::HitResult::Plain {
                        result = crate::orc::HitResult::Stun;
                    }
                }
            }
        }
        info!("strike #{hits}: {:?} -> stun {:.1}s quick {:.1}s down {:.1}s, hp {:.0}", result, d.stun, d.quick, d.down, d.hp);
        d.hit = Some(crate::orc::Hit { from, swing, heavy, kill: d.hp <= 0.0, result, leg });
        if !d.animated {
            let away = Vec3::new(d.pos.x - from.x, 0.0, d.pos.z - from.z).normalize_or_zero();
            d.push = away * power;
        }
    }
}

#[derive(Component)]
struct LockMarker;

/// A ring on the ground under the locked target.
fn spawn_lock_marker(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.spawn((
        Name::new("Lock-on marker"),
        Mesh3d(meshes.add(Annulus::new(0.62, 0.7))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.9, 0.78, 0.4),
            emissive: LinearRgba::rgb(2.4, 1.8, 0.6),
            unlit: true,
            cull_mode: None,
            ..default()
        })),
        Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        Visibility::Hidden,
        LockMarker,
    ));
}

fn update_lock_marker(
    dummies: Res<Dummies>,
    time: Res<Time>,
    mut marker: Query<(&mut Transform, &mut Visibility), With<LockMarker>>,
) {
    let Ok((mut transform, mut visibility)) = marker.single_mut() else { return };
    match dummies.1.and_then(|i| dummies.0.get(i)) {
        Some(d) => {
            let pulse = 1.0 + 0.05 * (time.elapsed_secs() * 6.0).sin();
            transform.translation = Vec3::new(d.pos.x, 0.03, d.pos.z);
            transform.scale = Vec3::splat(pulse);
            *visibility = Visibility::Inherited;
        }
        None => *visibility = Visibility::Hidden,
    }
}

/// Brief freeze when a blow lands (real seconds left), as the game does to sell an impact.
#[derive(Resource, Default)]
pub struct HitStop(pub f32);

/// Where blows landed this frame (world point, heavy), turned into sparks.
#[derive(Resource, Default)]
pub struct ImpactFx(pub Vec<(Vec3, bool)>);

#[derive(Component)]
struct Spark {
    velocity: Vec3,
    life: f32,
}

pub fn apply_hit_stop(real: Res<Time<Real>>, mut virtual_time: ResMut<Time<Virtual>>, mut stop: ResMut<HitStop>) {
    if stop.0 > 0.0 {
        stop.0 -= real.delta_secs();
        // A hit-stop is a hitch, not a freeze: the pose keeps creeping so the swing still reads as flowing.
        virtual_time.set_relative_speed(0.2);
    } else {
        virtual_time.set_relative_speed(1.0);
    }
}

pub fn spawn_sparks(
    mut commands: Commands,
    mut fx: ResMut<ImpactFx>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: Local<Option<(Handle<Mesh>, Handle<StandardMaterial>)>>,
) {
    if fx.0.is_empty() {
        return;
    }
    let (mesh, material) = cache
        .get_or_insert_with(|| {
            (
                meshes.add(Cuboid::new(0.025, 0.025, 0.09)),
                materials.add(StandardMaterial {
                    base_color: Color::srgb(1.0, 0.8, 0.4),
                    emissive: LinearRgba::rgb(9.0, 5.0, 1.4),
                    unlit: true,
                    ..default()
                }),
            )
        })
        .clone();
    for (point, heavy) in fx.0.drain(..) {
        let count = if heavy { 18 } else { 11 };
        for k in 0..count {
            // A deterministic fan of sparks: no RNG needed.
            let a = k as f32 * 2.399;
            let v = Vec3::new(a.cos(), 0.35 + 0.65 * ((k * 7 % 5) as f32 / 5.0), a.sin()).normalize() * (2.5 + (k % 4) as f32);
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                Transform::from_translation(point).looking_to(v, Vec3::Y),
                Spark { velocity: v, life: 0.28 + 0.03 * (k % 3) as f32 },
            ));
        }
    }
}

pub fn update_sparks(mut commands: Commands, time: Res<Time>, mut sparks: Query<(Entity, &mut Transform, &mut Spark)>) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut spark) in &mut sparks {
        spark.life -= dt;
        if spark.life <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        spark.velocity.y -= 9.0 * dt;
        transform.translation += spark.velocity * dt;
        transform.scale = Vec3::splat((spark.life * 4.0).min(1.0));
    }
}
