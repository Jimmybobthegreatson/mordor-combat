//! Talion's melee attacks as the game's database defines them (`Combat/Pool` -> `Combat/Attack`).
//!
//! Freeflow picks from *pools*: `PC_PoolAttacks_HC_<tier>_D<n>`. The tier (`Lowest`, `Low`, `Medium`,
//! `High`) is gated by the hit counter (the combo count), `D<n>` is the distance class. Each attack
//! record names its animation clip, the foot stance it starts from (predicate) and ends in (data), and
//! the directions it may be used in.

use crate::gamedb::{Entry, GameDb, Value};

const POOL_ATTACKS: u32 = 0xe9b6_32db;
const POOL_PREDICATE: u32 = 0xf6bb_663f;
const POOL_HIT_COUNTER: u32 = 0x0d34_9e9c;
const HIT_MIN: u32 = 0x00a7_a83a;
const HIT_MAX: u32 = 0x00a7_8b8c;
const ATTACK_ANIMATION: u32 = 0xa7fe_524e;
const ATTACK_DATA: u32 = 0x8a2b_a672;
const ATTACK_PREDICATE: u32 = 0xf6bb_663f;
const ANIMATION_CLIP: u32 = 0x87b8_ee23;
const DATA_END_STANCE: u32 = 0x96d3_1720;
const DATA_KIND: u32 = 0x6b38_026f;
const DATA_SCALING: u32 = 0x3ea0_3c4e;
const PREDICATE_DIRECTIONS: u32 = 0xa948_055f;
const PREDICATE_START_STANCE: u32 = 0x96d3_1720;
const PREDICATE_DISTANCE: u32 = 0xc90b_117d;

#[derive(Debug, Clone)]
pub struct PcAttack {
    /// The attack record, e.g. `PC_HC_Lowest_L1_D1_F_LL_RR_3`.
    pub name: String,
    /// The pool it belongs to, e.g. `PC_PoolAttacks_HC_Lowest_D1`.
    pub pool: String,
    /// Hit counter range (inclusive) in which the pool is available.
    pub hits: (u32, u32),
    pub clip: String,
    /// Foot stance it starts from and ends in: `LL`, `LR`, `RL`, `RR`.
    pub start: String,
    pub end: String,
    /// Which sides of the player the target may be on, e.g. `Forward`, `ForwardLeftRight`.
    pub allowed: String,
    /// Distance class `0..=3`.
    pub distance: u8,
    /// Target distance range (cm) in which the attack may be chosen, e.g. `(75.0, 250.0)` for class 1.
    pub range_cm: (f32, f32),
    /// Swing kind, e.g. `Light_RightToLeft`.
    pub kind: String,
    /// Root-motion scaling record, e.g. `PC_PoolAttacks_Short`.
    pub scaling: String,
}

fn stance_code(name: &str) -> String {
    // `LeftFootLeftShoulder_Forward` -> LL, `RightFootLeftShoulder_Forward` -> RL, ...
    let foot = if name.starts_with("Left") { 'L' } else { 'R' };
    let shoulder = match name.split("Foot").nth(1) {
        Some(rest) if rest.starts_with("Left") => 'L',
        _ => 'R',
    };
    format!("{foot}{shoulder}")
}

/// The single record `field` of `entry` points at, however the reference is encoded.
fn reference<'a>(db: &'a GameDb, entry: &Entry<'a>, field: u32) -> Option<Entry<'a>> {
    match entry.field(field)? {
        Value::Refs(r) | Value::Raw { values: r, .. } => r.first().and_then(|&v| db.follow(v)),
        Value::Strings(_) => None,
    }
}

fn name_of(db: &GameDb, entry: &Entry<'_>, field: u32) -> String {
    reference(db, entry, field).and_then(|e| e.name()).unwrap_or_default()
}

fn number(entry: &Entry<'_>, field: u32) -> Option<u32> {
    match entry.field(field)? {
        Value::Raw { values, .. } => values.first().copied(),
        _ => None,
    }
}

/// Every player melee attack of the pools `PC_PoolAttacks_HC_*` (the base game; DLC variants skipped).
pub fn pc_attacks(db: &GameDb) -> Vec<PcAttack> {
    let Some(pool_type) = (0..db.type_count()).find(|&t| db.type_info(t).is_some_and(|i| i.0 == "Combat/Pool")) else {
        return Vec::new();
    };
    let (_, structs, records) = db.type_info(pool_type).unwrap();
    let mut out = Vec::new();
    for i in structs..structs + records {
        let Some(pool) = db.item(pool_type, i) else { continue };
        let name = pool.name().unwrap_or_default();
        if !name.starts_with("PC_PoolAttacks_HC_") || name.contains("DLC") {
            continue;
        }
        let distance = name
            .split("_D")
            .nth(1)
            .and_then(|s| s.chars().next())
            .and_then(|c| c.to_digit(10))
            .unwrap_or(0) as u8;
        let hits = reference(db, &pool, POOL_PREDICATE)
            .and_then(|p| reference(db, &p, POOL_HIT_COUNTER))
            .map(|h| (number(&h, HIT_MIN).unwrap_or(0), number(&h, HIT_MAX).unwrap_or(9999)))
            .unwrap_or((0, 9999));
        let Some(Value::Raw { values, .. }) = pool.field(POOL_ATTACKS) else { continue };
        for attack in values.iter().filter_map(|&v| db.follow(v)) {
            let animation = reference(db, &attack, ATTACK_ANIMATION);
            let data = reference(db, &attack, ATTACK_DATA);
            let predicate = reference(db, &attack, ATTACK_PREDICATE);
            let (Some(animation), Some(data), Some(predicate)) = (animation, data, predicate) else { continue };
            let Some(Value::Strings(clip)) = animation.field(ANIMATION_CLIP) else { continue };
            let Some(clip) = clip.into_iter().next().filter(|c| !c.is_empty()) else { continue };
            out.push(PcAttack {
                name: attack.name().unwrap_or_default(),
                pool: name.clone(),
                hits,
                clip,
                start: stance_code(&name_of(db, &predicate, PREDICATE_START_STANCE)),
                end: stance_code(&name_of(db, &data, DATA_END_STANCE)),
                allowed: name_of(db, &predicate, PREDICATE_DIRECTIONS),
                distance: {
                    let d = name_of(db, &predicate, PREDICATE_DISTANCE);
                    d.rsplit('_').next().and_then(|s| s.parse().ok()).unwrap_or(distance)
                },
                range_cm: reference(db, &predicate, PREDICATE_DISTANCE)
                    .and_then(|d| d.raw_values().first().copied())
                    .and_then(|o| Some((db.pool_f32(o)?, db.pool_f32(o + 4)?)))
                    .unwrap_or((0.0, 10_000.0)),
                kind: name_of(db, &data, DATA_KIND),
                scaling: name_of(db, &data, DATA_SCALING),
            });
        }
    }
    out
}
