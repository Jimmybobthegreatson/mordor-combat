//! Which animation banks the combat loads. Shared by the game and by `som audit`, so the audit checks exactly what plays.

/// Talion's combat banks (the locomotion banks come first, from the game).
pub fn player_combat() -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for d in 1..=3 {
        for stance in ["ll", "lr", "rl", "rr"] {
            paths.push(format!("animation/player/combatattackpool/attackpool_l1/pl_l1_d{d}_{stance}.anix"));
        }
    }
    for d in 1..=3 {
        for stance in ["ll", "rr"] {
            paths.push(format!("animation/player/cmbt_newl2l3lightchain/elf_lt_l2_d{d}_{stance}.anix"));
            paths.push(format!("animation/player/cmbt_newl2l3lightchain/elf_lt_l3_d{d}_{stance}_stab.anix"));
        }
    }
    for bank in ["elf_newheavy", "elf_d0_l1", "elf_d0_l2", "pl_heavystabd0", "pl_heavystabd1", "pl_heavystabd2", "pl_heavystabd3"] {
        paths.push(format!("animation/player/elfmisc/{bank}.anix"));
    }
    for path in EXTRA_PLAYER {
        paths.push(path.to_string());
    }
    paths
}

const EXTRA_PLAYER: &[&str] = &[
    "animation/player/cmbt_pool_variation/combat_pool_variation.anix",
    "animation/player/cmbt_getthere/elf_gettheres.anix",
    "animation/player/elfmisc/elf_block.anix",
    "animation/player/elfmisc/elf_recoils.anix",
    "animation/player/elfmisc/pl_syncaction_counter.anix",
    "animation/player/elfmisc/elf_combat_flurry.anix",
    "animation/player/elfmisc/pl_syncaction_stealthkill.anix",
    "animation/player/elfmisc/elf_combatmisc.anix",
    "animation/player/cmbt_stunpool/pl_stunattackpool.anix",
    "animation/player/wulfmadeanims/pl_knockdownlegsweep.anix",
    "animation/player/elfmisc/pl_syncaction_chordkill_stand.anix",
    "animation/player/elfmisc/pl_syncaction_chordkill_onground.anix",
    // The vault over an enemy (`PC_DashOver`).
    "animation/player/elfmisc/elf_sync.anix",
];

/// The orcs' banks.
pub const ORC: &[&str] = &[
    "animation/shared/orc/orc_syncaction_chordkill_stand.anix",
    "animation/shared/orc/orc_syncaction_chordkill_onground.anix",
    "animation/infantry/orc/orc_inf_behavior.anix",
    "animation/infantry/orc/orc_inf_combat_attack.anix",
    "animation/shared/global/bip_recoils.anix",
    "animation/shared/orc/orc_combat_block.anix",
    "animation/shared/global/bip_deathposes.anix",
    "animation/shared/orc/orc_awareness.anix",
    "animation/shared/orc/orc_syncaction_counter.anix",
    "animation/shared/orc/orc_syncaction_stealthkill.anix",
    "animation/shared/orc/orc_syncaction.anix",
];
