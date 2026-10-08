//! Talion's moves as the game's database arranges them: **nodes** (what an input can start) -> **pools** -> **attacks**.
//!
//! * `Combat/Node` - one per kind of move. Its *type* is the priority, its *predicate* names the `input` that starts
//!   it (`Light`, `Counter`, `Dash`, ...), and its `Pools` hold the candidate attacks.
//! * `Combat/Pool` - attacks sharing a pool predicate (the hit-counter range of the freeflow pools).
//! * `Combat/Attack` - the move: its animation clip, the attack predicate (item, target state list, distance,
//!   allowed directions, stance, `AttackExplicitValue`, required enemy parry window) and, for synchronised moves,
//!   the `Combat/Sync` record that pairs it with the victim's attack.
//!
//! The engine evaluates these predicates with two functions whose debug strings survive in the exe
//! (`EvaluteTarget:*`, `EvaluteSelf:*`): target distance, vertical offset, **parry window open**, **can counter**,
//! line of sight, health range, character type, items, directions and combat states are checked in that order.

use std::collections::HashMap;

use anyhow::{Result, anyhow};

use crate::gamedb::{Entry, GameDb, Value};
use crate::hash::name_hash;

/// A state an attack puts its victim in (`Combat/Category.AppliedState`): the game's stun / knock-down rules.
#[derive(Debug, Clone)]
pub struct Applied {
    /// `Stunned`, `Quick_Stunned` or `KnockDown`.
    pub state: String,
    /// `BaseChance` (0..1).
    pub chance: f32,
    /// Seconds the state lasts: the record's `Duration` is a range in the float pool, this is its low end...
    pub duration: f32,
    /// ...and this its high end.
    pub duration_max: f32,
    /// Hit-counter range the category applies in (counter outcomes carry one category per tier).
    pub hits: (u32, u32),
}

/// A move the player can make.
#[derive(Debug, Clone)]
pub struct AttackDef {
    pub name: String,
    /// The node it belongs to and that node's priority.
    pub node: String,
    pub priority: u32,
    /// Animation clip the player plays.
    pub clip: String,
    /// For synchronised moves: the victim's clip.
    pub victim_clip: Option<String>,
    /// Foot stance it starts from and ends in (`LL`, `LR`, `RL`, `RR`), empty when it has none.
    pub start: String,
    pub end: String,
    /// Sides of the target the player may be on: words of `Forward`, `Left`, `Right`, `Back`.
    pub allowed: String,
    /// Sides of the player the attacker may be on (counters).
    pub attacker_sides: String,
    /// Target distance range in cm.
    pub range_cm: (f32, f32),
    /// `AttackExplicitValue`: among valid candidates the highest wins; `-1` (all bits set) is the lowest.
    pub explicit: i32,
    /// Item the player must carry: `PC_Sword`, `PC_SwordOrDagger`, ... (empty = any).
    pub item: String,
    /// Combat-state list the target must be in (`Quick_Stunned`, `NormalOrStunned_NotQuickStnd`, ...).
    pub target_states: String,
    /// Required parry state of the target's current attack (`PC_ParrySuccess` for counters).
    pub parry: String,
    pub require_target: bool,
    /// Items the player must / must not carry (`CSP_HeadExp` is the head-explosion upgrade).
    pub needs_item: String,
    pub forbids_item: String,
    /// Items the target must / must not carry (`Biped_Resistant_Combat`, `Named_Enemy`, ...).
    pub target_needs: String,
    pub target_forbids: String,
    /// Health the target must have (absolute points), e.g. the finisher's `PC_FlurryEnder`.
    pub health_range: Option<(f32, f32)>,
    /// Hit-counter range of the pool it comes from.
    pub hits: (u32, u32),
    /// Swing kind of the move (`Light_RightToLeft`, ...).
    pub kind: String,
    /// What it does to the victim (stun / knock-down), from the attack's categories.
    pub applied: Vec<Applied>,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub priority: u32,
    pub input: String,
    /// Indices into [`Moves::attacks`].
    pub attacks: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Moves {
    pub nodes: Vec<Node>,
    pub attacks: Vec<AttackDef>,
    /// Attack name -> index in `attacks` (first node that holds it).
    pub by_name: HashMap<String, usize>,
}

impl Moves {
    /// The states the named attack applies to its victim.
    pub fn applied(&self, name: &str) -> &[Applied] {
        self.by_name.get(name).map(|&i| self.attacks[i].applied.as_slice()).unwrap_or(&[])
    }
}

/// Node name prefixes that are ground melee for the player (the rest of `PC_*` is grabs, mounts, ledges...).
const WANTED: [&str; 7] =
    ["PC_PoolAttacks", "PC_Flurry_Standing_", "PC_Counter", "PC_InstaKill_Sneak", "PC_DashAttack", "PC_Dash", "PC_Stun"];

fn field(e: &Entry<'_>, name: &str) -> Option<Value> {
    e.field(name_hash(name))
}

fn number(e: &Entry<'_>, name: &str) -> Option<u32> {
    match field(e, name)? {
        Value::Raw { values, .. } => values.first().copied(),
        _ => None,
    }
}

fn string(e: &Entry<'_>, name: &str) -> Option<String> {
    match field(e, name)? {
        Value::Strings(s) => s.into_iter().next(),
        _ => None,
    }
}

fn refs(e: &Entry<'_>, name: &str) -> Vec<u32> {
    match field(e, name) {
        Some(Value::Refs(r)) | Some(Value::Raw { values: r, .. }) => r,
        _ => Vec::new(),
    }
}

fn first_ref<'a>(db: &'a GameDb, e: &Entry<'a>, name: &str) -> Option<Entry<'a>> {
    refs(e, name).first().and_then(|&r| db.follow(r))
}

fn name_of(db: &GameDb, e: &Entry<'_>, field_name: &str) -> String {
    first_ref(db, e, field_name).and_then(|x| x.name()).unwrap_or_default()
}

fn stance_code(name: &str) -> String {
    if name.is_empty() || !name.contains("Foot") {
        return String::new();
    }
    let foot = if name.starts_with("Left") { 'L' } else { 'R' };
    let shoulder = match name.split("Foot").nth(1) {
        Some(rest) if rest.starts_with("Left") => 'L',
        _ => 'R',
    };
    format!("{foot}{shoulder}")
}

fn type_index(db: &GameDb, name: &str) -> Option<usize> {
    (0..db.type_count()).find(|&t| db.type_info(t).is_some_and(|i| i.0 == name))
}

/// Read every wanted `Combat/Node` and the attacks of its pools.
pub fn load(db: &GameDb) -> Result<Moves> {
    let node_t = type_index(db, "Combat/Node").ok_or_else(|| anyhow!("no Combat/Node"))?;
    let (_, nf, nr) = db.type_info(node_t).unwrap();

    // Sync records: source (player) attack -> target (victim) attack.
    let mut sync_target: HashMap<u32, u32> = HashMap::new();
    if let Some(sync_t) = type_index(db, "Combat/Sync") {
        let (_, f, r) = db.type_info(sync_t).unwrap();
        for i in f..f + r {
            let Some(e) = db.item(sync_t, i) else { continue };
            if let (Some(&src), Some(&dst)) = (refs(&e, "SourceAttack").first(), refs(&e, "TargetAttack").first()) {
                sync_target.insert(src, dst);
            }
        }
    }

    let mut moves = Moves::default();
    for i in nf..nf + nr {
        let Some(node) = db.item(node_t, i) else { continue };
        let name = node.name().unwrap_or_default();
        // `PC_InstaKill_KBM` is the keyboard Execution (input `TokenKill_KBM`): the one `_KBM` node kept.
        let execution = name == "PC_InstaKill_KBM";
        if !execution && (!WANTED.iter().any(|p| name.starts_with(p)) || name.contains("DLC") || name.ends_with("_OLD") || name.ends_with("_KBM")) {
            continue;
        }
        let priority = number(&node, "type").unwrap_or(0);
        let input = first_ref(db, &node, "predicate")
            .map(|p| name_of(db, &p, "input"))
            .unwrap_or_default();
        let mut indices = Vec::new();
        for pool_ref in refs(&node, "Pools") {
            let Some(pool) = db.follow(pool_ref) else { continue };
            let hits = first_ref(db, &pool, "PREDICATE")
                .and_then(|p| first_ref(db, &p, "HitCounterRange"))
                .map(|h| (number(&h, "MIN").unwrap_or(0), number(&h, "Max").unwrap_or(9999)))
                .unwrap_or((0, u32::MAX));
            for &attack_ref in &refs(&pool, "Attacks") {
                let Some(attack) = db.follow(attack_ref) else { continue };
                let (Some(anim), Some(data), Some(pred)) =
                    (first_ref(db, &attack, "AnimationData"), first_ref(db, &attack, "AttackData"), first_ref(db, &attack, "predicate"))
                else {
                    continue;
                };
                let Some(clip) = string(&anim, "name").filter(|c| !c.is_empty()) else { continue };
                let victim_clip = sync_target
                    .get(&attack_ref)
                    .and_then(|&t| db.follow(t))
                    .and_then(|t| first_ref(db, &t, "AnimationData").and_then(|a| string(&a, "name")).filter(|c| !c.is_empty()))
                    // Pairs not listed in `Combat/Sync` follow the naming: `PL_Bip_Sync_X` <-> `Bip_PL_Sync_X`.
                    .or_else(|| clip.strip_prefix("PL_Bip_Sync_").map(|rest| format!("Bip_PL_Sync_{rest}")));
                let range_cm = first_ref(db, &pred, "distance")
                    .and_then(|d| d.raw_values().first().copied())
                    .and_then(|o| Some((db.pool_f32(o)?, db.pool_f32(o + 4)?)))
                    .unwrap_or((0.0, 10_000.0));
                let applied = applied_states(db, &attack);
                indices.push(moves.attacks.len());
                moves.by_name.entry(attack.name().unwrap_or_default()).or_insert(moves.attacks.len());
                moves.attacks.push(AttackDef {
                    name: attack.name().unwrap_or_default(),
                    node: name.clone(),
                    priority,
                    clip,
                    victim_clip,
                    start: stance_code(&name_of(db, &pred, "AttackStance")),
                    end: stance_code(&name_of(db, &data, "AttackStance")),
                    allowed: name_of(db, &pred, "AllowedDirection"),
                    attacker_sides: name_of(db, &pred, "DirectionAllowedToAttacker"),
                    range_cm,
                    explicit: number(&pred, "AttackExplicitValue").map_or(0, |v| v as i32),
                    item: name_of(db, &pred, "item"),
                    target_states: name_of(db, &pred, "StateTargetList"),
                    parry: name_of(db, &pred, "TargetParryType"),
                    require_target: number(&pred, "RequireTarget").unwrap_or(0) != 0,
                    needs_item: name_of(db, &pred, "HasInventoryItem"),
                    forbids_item: name_of(db, &pred, "DoesNotHaveInventoryItem"),
                    target_needs: name_of(db, &pred, "TargetHasInventoryItem"),
                    target_forbids: name_of(db, &pred, "TargetDoesNotHaveInvItem"),
                    health_range: first_ref(db, &pred, "TargetHealthRange")
                        .and_then(|h| h.raw_values().first().copied())
                        .and_then(|o| Some((db.pool_f32(o)?, db.pool_f32(o + 4)?))),
                    hits,
                    kind: name_of(db, &data, "RecoilTypeHit"),
                    applied,
                });
            }
        }
        moves.nodes.push(Node { name, priority, input, attacks: indices });
    }
    Ok(moves)
}

fn float(db: &GameDb, e: &Entry<'_>, name: &str) -> Option<f32> {
    e.raw_field(name_hash(name))?.first().and_then(|&o| db.pool_f32(o))
}

/// The stun / knock-down states an attack's categories apply, with chance, duration and hit-counter range.
fn applied_states(db: &GameDb, attack: &Entry<'_>) -> Vec<Applied> {
    let mut out = Vec::new();
    for category in refs(attack, "category").iter().filter_map(|&r| db.follow(r)) {
        let hits = first_ref(db, &category, "PredicateHitCounterRange")
            .map(|h| (number(&h, "MIN").unwrap_or(0), number(&h, "Max").unwrap_or(u32::MAX)))
            .unwrap_or((0, u32::MAX));
        for record in refs(&category, "AppliedState").iter().filter_map(|&r| db.follow(r)) {
            let state = name_of(db, &record, "State");
            if !matches!(state.as_str(), "Stunned" | "Quick_Stunned" | "KnockDown") {
                continue;
            }
            out.push(Applied {
                state,
                chance: match field(&record, "BaseChance") {
                    Some(Value::Raw { values, .. }) => values.first().map(|&v| f32::from_bits(v)).unwrap_or(1.0),
                    _ => 1.0,
                },
                duration: float(db, &record, "Duration").unwrap_or(0.0),
                duration_max: record
                    .raw_field(name_hash("Duration"))
                    .and_then(|r| r.first())
                    .and_then(|&o| db.pool_f32(o + 4))
                    .filter(|m| m.is_finite() && *m > 0.0 && *m < 600.0)
                    .or_else(|| float(db, &record, "Duration"))
                    .unwrap_or(0.0),
                hits,
            });
        }
    }
    out
}
