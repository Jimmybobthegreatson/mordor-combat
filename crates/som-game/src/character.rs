//! Loads Talion's skeleton and the animation banks from the install and spawns the skeleton as joint entities, drawn as a stick figure.
//! No character mesh, texture or material is read.

use bevy::prelude::*;
use som_formats::animator::Library;
use som_formats::gamedb::GameDb;
use som_formats::skel::Skeleton;
use som_formats::vfs::Vfs;

const SKELETON: &str = "models/player/player_talion/player_talion_pd.skel";
const GAMEDB: &str = "database/game/game.gamedb";
/// The sword's own skeleton (one bone line along the blade).
const SWORD_SKELETON: &str = "models/weapons/ranger_equipment/ranger_sword03/ranger_sword03.skel";

/// Game units are centimetres.
const CM: f32 = 0.01;

pub struct CharacterPlugin;

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_character).add_systems(Update, draw_sticks);
    }
}

/// The game is left-handed with Z forward. Mirroring Z gives Bevy's right-handed space with
/// the character facing -Z; a mirrored rotation keeps its angle and flips the x/y axis parts.
pub fn to_bevy_vec(v: [f32; 3]) -> Vec3 {
    Vec3::new(v[0], v[1], -v[2]) * CM
}

pub fn to_bevy_quat(q: [f32; 4]) -> Quat {
    Quat::from_xyzw(-q[0], -q[1], q[2], q[3])
}

/// The spawned skeleton: joint entities and the animation library that drives them.
#[derive(Resource)]
pub struct Rig {
    pub lib: Library,
    /// Joint entity of each skeleton bone.
    pub joints: Vec<Entity>,
    /// Parent of the skeleton; moved and turned by the player.
    pub player: Entity,
    /// Talion's melee attacks from the game database.
    pub attacks: Vec<som_formats::combatdb::PcAttack>,
    /// The game's move nodes and their attacks.
    pub moves: som_formats::combatnodes::Moves,
    /// The drawn sword in his hand (hidden while sheathed).
    pub sword: Entity,
    /// The sheathed sword on his back (nothing to show without a mesh).
    pub back_sword: Vec<Entity>,
    /// The dagger and the joints it moves between: his belt sheath and his hand.
    pub dagger: Entity,
    pub dagger_sheath: Entity,
    pub dagger_hand: Entity,
}

/// A joint drawn as a line to its parent.
#[derive(Component)]
pub struct StickJoint {
    color: Color,
    head: bool,
}

/// Stick figures: every bone as a line from its parent, the head as a ring. Helper and IK bones sit far from their parents and are skipped.
pub fn draw_sticks(mut gizmos: Gizmos, joints: Query<(&GlobalTransform, &ChildOf, &StickJoint, &InheritedVisibility)>, parents: Query<&GlobalTransform, With<StickJoint>>) {
    for (at, parent, joint, visible) in &joints {
        if !visible.get() {
            continue;
        }
        if joint.head {
            gizmos.sphere(at.translation(), 0.1, joint.color);
        }
        if let Ok(from) = parents.get(parent.parent()) {
            if from.translation().distance(at.translation()) < 0.5 {
                gizmos.line(from.translation(), at.translation(), joint.color);
            }
        }
    }
}

/// Spawn `skeleton`'s bones as joint entities under `root`, drawn in `color`.
pub fn spawn_stick(commands: &mut Commands, skeleton: &Skeleton, root: Entity, color: Color) -> Vec<Entity> {
    let mut joints: Vec<Entity> = Vec::with_capacity(skeleton.bones.len());
    for (i, bone) in skeleton.bones.iter().enumerate() {
        let (t, q) = skeleton.local_bind(i);
        let joint = commands
            .spawn((
                Name::new(bone.name.clone()),
                Transform { translation: to_bevy_vec(t), rotation: to_bevy_quat(q), scale: Vec3::ONE },
                Visibility::default(),
                StickJoint { color, head: bone.name == "Head" },
            ))
            .id();
        commands.entity(bone.parent.map_or(root, |p| joints[p])).add_child(joint);
        joints.push(joint);
    }
    joints
}

fn load_character(mut commands: Commands) {
    match try_load(&mut commands) {
        Ok(rig) => commands.insert_resource(rig),
        Err(err) => error!("could not load the player from the game install: {err:#}"),
    }
}

fn try_load(commands: &mut Commands) -> anyhow::Result<Rig> {
    let mut vfs = Vfs::open(Vfs::locate()?)?;
    let skeleton = Skeleton::parse(&vfs.read(SKELETON)?)?;
    let mut lib = Library::new(&skeleton)?;
    for bank in crate::combat::bank_paths() {
        lib.add_bank(&vfs.read(&bank)?)?;
    }
    // Lift the figure so its lowest bone rests on the plate.
    let lowest = skeleton.bones.iter().map(|b| b.translation[1]).fold(0.0f32, f32::min);
    let player = commands.spawn((Name::new("Player"), Transform::default(), Visibility::default())).id();
    let root = commands.spawn((Name::new("Talion"), Transform::from_xyz(0.0, -lowest * CM, 0.0), Visibility::default())).id();
    commands.entity(player).add_child(root);

    let db = GameDb::parse(vfs.read(GAMEDB)?)?;
    let attacks = som_formats::combatdb::pc_attacks(&db);
    let moves = som_formats::combatnodes::load(&db)?;
    let joints = spawn_stick(commands, &skeleton, root, Color::srgb(0.2, 0.9, 1.0));

    // The drawn sword rides in the right hand, hidden until combat.
    let sword = commands.spawn((Name::new("Sword"), Transform::default(), Visibility::Hidden)).id();
    match (|| -> anyhow::Result<()> {
        let hand = skeleton.find("R_Weapon").ok_or_else(|| anyhow::anyhow!("no R_Weapon bone"))?;
        let sword_skel = Skeleton::parse(&vfs.read(SWORD_SKELETON)?)?;
        spawn_stick(commands, &sword_skel, sword, Color::srgb(0.9, 0.9, 0.9));
        commands.entity(joints[hand]).add_child(sword);
        Ok(())
    })() {
        Ok(()) => {}
        Err(err) => warn!("sword unavailable: {err:#}"),
    }

    // The dagger has no stick of its own; the entity only keeps the stealth-kill bookkeeping working.
    let dagger = commands.spawn((Name::new("Dagger"), Transform::default(), Visibility::Hidden)).id();
    let dagger_sheath = joints[skeleton.find("Dagger_Sheath_Socket").or_else(|| skeleton.find("Dagger_Sheath")).unwrap_or(0)];
    let dagger_hand = joints[skeleton.find("L_Weapon").unwrap_or(0)];
    commands.entity(dagger_sheath).add_child(dagger);

    info!("loaded {} bones", skeleton.bones.len());
    Ok(Rig { lib, joints, player, attacks, moves, sword, back_sword: Vec::new(), dagger, dagger_sheath, dagger_hand })
}
