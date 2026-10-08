//! Checks against a real install. Skipped (pass trivially) when no install is found;
//! set `SOM_DIR` to the folder holding the `.arch05` files to run them.

use som_formats::anix::Anix;
use som_formats::math::{qdot, qmul, qrot, vadd};
use som_formats::skel::Skeleton;
use som_formats::vfs::Vfs;

const TALION_SKEL: &str = "models/player/player_talion/player_talion_pd.skel";
const TALION_MESH: &str = "models/player/player_talion/player_talion_pd.mesh";
const ATTACK_BANK: &str = "animation/player/combatattackpool/attackpool_l1/pl_l1_d1_ll.anix";

fn vfs() -> Option<Vfs> {
    let root = Vfs::locate().ok()?;
    Vfs::open(root).ok()
}

fn talion(vfs: &mut Vfs) -> Skeleton {
    Skeleton::parse(&vfs.read(TALION_SKEL).unwrap()).unwrap()
}

#[test]
fn skeleton_is_a_single_tree_of_unit_rotations() {
    let Some(mut vfs) = vfs() else { return };
    let skel = talion(&mut vfs);
    assert_eq!(skel.bones.len(), 168);
    assert_eq!(skel.bones[0].name, "Null");
    assert_eq!(skel.bones.iter().filter(|b| b.parent.is_none()).count(), 1);
    for (i, bone) in skel.bones.iter().enumerate() {
        assert!(bone.parent.is_none_or(|p| p < i), "{} comes before its parent", bone.name);
        assert!((qdot(bone.rotation, bone.rotation) - 1.0).abs() < 1e-3, "{} rotation not unit", bone.name);
    }
    // Head height in centimetres.
    let head = skel.find("Head").unwrap();
    assert!((skel.bones[head].translation[1] - 72.75).abs() < 0.5);
}

/// The bank's bind pose is parent-relative; walking it down the hierarchy must land on the
/// skeleton's model-space bind transforms. This exercises SV01, SQ03 and the node mapping.
#[test]
fn bank_bind_pose_matches_skeleton() {
    let Some(mut vfs) = vfs() else { return };
    let skel = talion(&mut vfs);
    let bank = Anix::parse(&vfs.read(ATTACK_BANK).unwrap()).unwrap();
    let pose = bank.bind_pose().unwrap();
    let world = world_pose(&skel, &bank, &pose);
    for &hash in &bank.node_hashes {
        let bone = skel.find_hash(hash).expect("bank node missing from skeleton");
        let b = &skel.bones[bone];
        let (t, q) = world[bone];
        for k in 0..3 {
            assert!((t[k] - b.translation[k]).abs() < 1.5, "{} position {:?} vs {:?}", b.name, t, b.translation);
        }
        assert!(qdot(q, b.rotation).abs() > 0.999, "{} rotation {:?} vs {:?}", b.name, q, b.rotation);
    }
}

#[test]
fn forward_attack_lunges_forward_and_stays_sane() {
    let Some(mut vfs) = vfs() else { return };
    let skel = talion(&mut vfs);
    let bank = Anix::parse(&vfs.read(ATTACK_BANK).unwrap()).unwrap();
    let clip = bank.clip("Elf_Lt_L1_D1_F_LL_RR_3").expect("clip missing");
    assert_eq!(clip.end_ms(), 2066);
    let bind = bank.bind_pose().unwrap();
    let pelvis = skel.find("Pelvis").unwrap();
    let foot = skel.find("L_Foot").unwrap();

    let mut last_z = f32::MIN;
    for step in 0..=20 {
        let t = clip.end_ms() * step / 20;
        let pose = bank.sample(clip, t, &bind).unwrap();
        for q in &pose.rotation {
            assert!((qdot(*q, *q) - 1.0).abs() < 1e-3, "non-unit rotation at {t} ms");
        }
        let world = world_pose(&skel, &bank, &pose);
        let z = world[pelvis].0[2];
        assert!(z.is_finite());
        // Root travel is monotonic to within a few centimetres of sway.
        assert!(z > last_z - 25.0, "pelvis jumped backwards at {t} ms: {last_z} -> {z}");
        last_z = last_z.max(z);
        // The foot never sinks far below the floor (about -92 cm in model space).
        assert!(world[foot].0[1] > -100.0, "foot under the floor at {t} ms");
    }
    let start = world_pose(&skel, &bank, &bank.sample(clip, 0, &bind).unwrap())[pelvis].0[2];
    assert!(last_z - start > 100.0, "forward attack only travelled {} cm", last_z - start);
}

/// Model-space transforms for every skeleton bone: bank nodes take the pose, the rest keep bind.
fn world_pose(skel: &Skeleton, bank: &Anix, pose: &som_formats::anix::Pose) -> Vec<([f32; 3], [f32; 4])> {
    let mut local: Vec<_> = (0..skel.bones.len()).map(|i| skel.local_bind(i)).collect();
    for (node, &hash) in bank.node_hashes.iter().enumerate() {
        if let Some(bone) = skel.find_hash(hash) {
            local[bone] = (pose.translation[node], pose.rotation[node]);
        }
    }
    let mut world: Vec<([f32; 3], [f32; 4])> = Vec::with_capacity(local.len());
    for (i, bone) in skel.bones.iter().enumerate() {
        let (t, q) = local[i];
        world.push(match bone.parent {
            None => (t, q),
            Some(p) => {
                let (pt, pq) = world[p];
                (vadd(pt, qrot(pq, t)), qmul(pq, q))
            }
        });
    }
    world
}

#[test]
fn locomotion_loops_travel_as_measured_and_stay_finite() {
    use som_formats::animator::Animator;
    let Some(mut vfs) = vfs() else { return };
    let lib = movement_library(&mut vfs);
    let clip = |bank, name| lib.clip(bank, name).unwrap_or_else(|| panic!("clip {name} missing"));
    let idle = clip(0, "PL_Base_L_Fist_IdleBase");
    let walk = clip(0, "PL_Cmbt_Walk");
    let run = clip(1, "PL_Base_L_Fist_RunAction");
    clip(2, "PL_Base_L_Fist_RunToIdle");

    for (id, min, max) in [(idle, -5.0, 5.0), (walk, 380.0, 480.0), (run, 700.0, 850.0)] {
        let mut animator = Animator::new();
        animator.play(&lib, id, true, 0.0, 0.0).unwrap();
        let duration = lib.duration_ms(id) / 1000.0;
        let steps = (duration * 60.0).round() as usize;
        let mut z = 0.0;
        for _ in 0..steps {
            let step = animator.update(&lib, 1.0 / 60.0).unwrap();
            z += step.travel[2];
            assert!(step.pose.iter().all(|b| b.t.iter().chain(&b.r).all(|v| v.is_finite())));
        }
        assert!((min..=max).contains(&z), "{} travelled {z} cm per cycle", lib.name(id));
    }

    // A cross-fade from walk into run keeps moving forward and never jumps.
    let mut animator = Animator::new();
    animator.play(&lib, walk, true, 0.0, 0.0).unwrap();
    for _ in 0..30 {
        animator.update(&lib, 1.0 / 60.0).unwrap();
    }
    animator.play(&lib, run, true, 0.25, animator.phase(&lib)).unwrap();
    for _ in 0..60 {
        let step = animator.update(&lib, 1.0 / 60.0).unwrap();
        assert!(step.travel[2] > 0.0 && step.travel[2] < 30.0, "frame travel {}", step.travel[2]);
    }
    assert_eq!(animator.current(), Some(run));
}

#[test]
fn attack_clips_carry_hit_cues() {
    let Some(mut vfs) = vfs() else { return };
    let bank = Anix::parse(&vfs.read(ATTACK_BANK).unwrap()).unwrap();
    let clip = bank.clip("Elf_Lt_L1_D1_F_LL_RR_3").unwrap();
    assert_eq!(clip.event_ms("APPLYDAMAGE"), Some(1400));
    assert_eq!(clip.event_ms("BHITWIN"), Some(630));
    assert_eq!(clip.event_ms("EHITWIN"), Some(1640));
    assert_eq!(clip.event_ms("BUFFTRANS"), Some(1640));
    // The blow lands inside the hit window, before the clip ends.
    for clip in bank.clips.iter().filter(|c| c.name != "bind") {
        let (hit, from, to) = (
            clip.event_ms("APPLYDAMAGE").unwrap(),
            clip.event_ms("BHITWIN").unwrap(),
            clip.event_ms("EHITWIN").unwrap(),
        );
        assert!(from <= hit && hit <= to && hit < clip.end_ms(), "{} hit {hit} window {from}..{to}", clip.name);
    }
}

#[test]
fn draw_and_stow_clips_carry_their_cues() {
    let Some(mut vfs) = vfs() else { return };
    let bank = Anix::parse(&vfs.read("animation/player/elfmisc/elf_combatmisc.anix").unwrap()).unwrap();
    // The blade leaves the scabbard at 450 ms and goes back in at 990 ms.
    assert_eq!(bank.clip("PL_Sword_UnSheathe_1").unwrap().event_ms("UNSHEATH"), Some(450));
    assert_eq!(bank.clip("PL_Sword_Sheathe_1").unwrap().event_ms("SHEATH"), Some(990));
}

#[test]
fn game_database_defines_talions_attack_pools() {
    use som_formats::combatdb::pc_attacks;
    use som_formats::gamedb::GameDb;
    let Some(mut vfs) = vfs() else { return };
    let db = GameDb::parse(vfs.read("database/game/game.gamedb").unwrap()).unwrap();
    let attacks = pc_attacks(&db);
    assert!(attacks.len() > 500, "{} attacks", attacks.len());
    // The hit counter gates four tiers.
    let mut tiers: Vec<(u32, u32)> = attacks.iter().map(|a| a.hits).collect();
    tiers.sort();
    tiers.dedup();
    assert_eq!(tiers, [(0, 3), (4, 15), (16, 29), (30, 9999)]);
    let a = attacks.iter().find(|a| a.name == "PC_HC_Lowest_L1_D1_F_LL_RR_3").unwrap();
    assert_eq!((a.clip.as_str(), a.start.as_str(), a.end.as_str()), ("Elf_Lt_L1_D1_F_LL_RR_3", "LL", "RR"));
    assert_eq!(a.allowed, "Forward");
    // Distance classes are centimetre ranges to the target.
    assert_eq!(a.range_cm, (75.0, 250.0));
    let far = attacks.iter().find(|a| a.distance == 3).unwrap();
    assert_eq!(far.range_cm.1, 700.0);
}

#[test]
fn move_nodes_follow_the_games_priorities() {
    use som_formats::combatnodes::load;
    use som_formats::gamedb::GameDb;
    let Some(mut vfs) = vfs() else { return };
    let db = GameDb::parse(vfs.read("database/game/game.gamedb").unwrap()).unwrap();
    let moves = load(&db).unwrap();
    let node = |n: &str| moves.nodes.iter().find(|x| x.name == n).unwrap_or_else(|| panic!("no node {n}"));
    // Counter outranks its own failure node, and the freeflow nodes answer the Light input.
    assert_eq!((node("PC_Counter_Basic").input.as_str(), node("PC_Counter_Basic").priority), ("Counter", 4));
    assert_eq!((node("PC_Counter_Fail").input.as_str(), node("PC_Counter_Fail").priority), ("Counter", 13));
    assert_eq!(node("PC_PoolAttacks").input, "Light");
    // A counter needs the enemy's parry window and pairs with the victim's clip.
    let counter = &moves.attacks[node("PC_Counter_Basic").attacks[0]];
    assert_eq!(counter.parry, "PC_ParrySuccess");
    assert!(counter.victim_clip.is_some());
    // The finisher needs a quick-stunned target, and ranks above the flurry hits.
    let end = node("PC_Flurry_Standing_END").attacks.iter().map(|&i| &moves.attacks[i]).find(|a| a.name == "PC_StandingFlurry_END_Kill_F").unwrap();
    assert_eq!(end.target_states, "Quick_Stunned");
    assert!(end.explicit > moves.attacks[node("PC_Flurry_Standing_2").attacks[0]].explicit);
}

#[test]
fn stun_and_knockdown_durations_come_from_the_database() {
    use som_formats::combatnodes::load;
    use som_formats::gamedb::GameDb;
    let Some(mut vfs) = vfs() else { return };
    let db = GameDb::parse(vfs.read("database/game/game.gamedb").unwrap()).unwrap();
    let moves = load(&db).unwrap();
    let node = |n: &str| moves.nodes.iter().find(|x| x.name == n).unwrap_or_else(|| panic!("no node {n}"));
    let applied = |n: &str| moves.attacks[node(n).attacks[0]].applied.clone();
    // Knock-down sweeps (for a plain-stunned target) knock down for 3 s at every tier.
    let kd = applied("PC_PoolAttacks_KD");
    assert!(kd.iter().all(|a| a.state == "KnockDown" && (a.duration - 3.0).abs() < 1e-3 && a.chance == 1.0));
    assert!(moves.attacks[node("PC_PoolAttacks_KD").attacks[0]].target_states == "StunnedOnly");
    // The stun counter's stun grows with the hit counter: 0.5, 1.0, 1.5, 2.0 s.
    let mut stun = applied("PC_Counter_Stun");
    stun.sort_by_key(|a| a.hits.0);
    let seconds: Vec<f32> = stun.iter().map(|a| a.duration).collect();
    assert_eq!(seconds, [0.5, 1.0, 1.5, 2.0]);
    // Freeflow pool hits: the low tier stuns 2 s with 75% chance.
    let low = moves.attacks.iter().find(|a| a.node == "PC_PoolAttacks" && a.name.starts_with("PC_HC_Low_")).unwrap();
    assert!(low.applied.iter().any(|a| a.state == "Stunned" && (a.duration - 2.0).abs() < 1e-3 && (a.chance - 0.75).abs() < 1e-3));
}

#[test]
fn light_pool_attacks_have_their_clips_and_cues() {
    use som_formats::animator::Library;
    use som_formats::gamedb::GameDb;
    let Some(mut vfs) = vfs() else { return };
    let db = GameDb::parse(vfs.read("database/game/game.gamedb").unwrap()).unwrap();
    let moves = som_formats::combatnodes::load(&db).unwrap();
    let skeleton = Skeleton::parse(&vfs.read("models/player/player_talion/player_talion_pd.skel").unwrap()).unwrap();
    let mut lib = Library::new(&skeleton).unwrap();
    for bank in som_formats::banks::player_combat() {
        lib.add_bank(&vfs.read(&bank).unwrap()).unwrap();
    }
    let node = moves.nodes.iter().find(|n| n.name == "PC_PoolAttacks").unwrap();
    let (mut loaded, mut missing) = (0, 0);
    for a in node.attacks.iter().map(|&i| &moves.attacks[i]) {
        // Every freeflow attack shares one explicit value: the game picks among the valid ones by variation alone.
        assert!(a.explicit == 20 || a.name == "PL_LedgeKick", "{} has value {}", a.name, a.explicit);
        match lib.find(&a.clip) {
            None => missing += 1,
            Some(clip) => {
                loaded += 1;
                let open = lib.event_s(clip, "BCOL");
                let damage = lib.event_s(clip, "APPLYDAMAGE");
                assert!(open.is_some() && damage.is_some(), "{} lacks its collider or damage cue", a.clip);
                assert!(open.unwrap() <= damage.unwrap() + 0.02, "{} opens its collider after the damage cue", a.clip);
            }
        }
    }
    assert!(loaded > 500 && missing <= 4, "{loaded} loaded, {missing} missing");
}


fn movement_library(vfs: &mut Vfs) -> som_formats::animator::Library {
    let skeleton = talion(vfs);
    let mut lib = som_formats::animator::Library::new(&skeleton).unwrap();
    for bank in MOVEMENT_BANKS {
        lib.add_bank(&vfs.read(bank).unwrap()).unwrap();
    }
    lib
}

const MOVEMENT_BANKS: [&str; 3] = [
    "animation/player/coremovement/pl_movbase.anix",
    "animation/player/coremovement/pl_movaction.anix",
    "animation/player/coremovement/pl_transbase.anix",
];
