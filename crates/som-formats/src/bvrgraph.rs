//! The player behaviour graph (`behaviors/player/player_elf.bvr`, a GADB) as a state machine: `BlendState` records with the
//! clip tree they play, and their `Links` (`StateTransition` records) gated by predicate trees. Everything here is read from the
//! game's data; the values only the engine knows (stick magnitude, the turn angle, ...) and the world queries (context action
//! lines, shape casts) come from an [`Env`] the caller supplies.
//!
//! Records refer to each other by name hash (kind 10) or by `type << 20 | item` (kind 11); both are followed.

use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::gamedb::{Entry, GameDb, Value};
use crate::hash::name_hash;

/// A `LogicNodeData.ContextActionData`: how the game searches its context-action lines (`som_formats::cntx`) for one type.
#[derive(Debug, Clone, Default)]
pub struct ContextQuery {
    pub name: String,
    /// `GameDBContextAction`: the line type searched for.
    pub action: String,
    pub max_dist: f32,
    pub min_dist: f32,
    pub approach_angle: f32,
    pub test_angle_rotation: f32,
    pub los_angle: f32,
    pub max_connection_dist: f32,
    pub max_connection_angle: f32,
    /// Every cell the exe's search reads.
    pub data: crate::ctxsearch::Data,
    /// `TestAngleRotationOverride` / `TestAngleRotationWorldOverride`: the named value to evaluate (radians), if any.
    pub test_override: Option<String>,
    pub world_override: Option<String>,
    /// `QueryWorldPosition`: the named point the graph holds.
    pub query_position: Option<String>,
    /// `ContextActionOutput`: the named values the graph reads after a successful search.
    pub output: Option<ContextOutput>,
}

/// `ShapeCastData`: a capsule swept from the character (plus `offset`) to a target (`FUN_14089d890` builds the segment).
#[derive(Debug, Clone, Default)]
pub struct ShapeCast {
    pub radius: f32,
    pub height: f32,
    pub offset: [f32; 3],
    pub target_offset: [f32; 3],
    pub flatten_y: bool,
    pub local: bool,
    pub directional: bool,
    /// The named world point the cast aims at (`TargetWorldPosition`), and the values of `TargetX/Y/Z`.
    pub target_world: Option<String>,
    pub target_x: Option<String>,
    pub target_y: Option<String>,
    pub target_z: Option<String>,
}

/// A `RayCastData` record: the ray runs from `source` (a named point, else the character) plus `offset` to `target` (a named point, else the start)
/// plus `target_offset`; offsets in cm, turned by the character's yaw.
#[derive(Debug, Clone, Default)]
pub struct RayCast {
    pub name: String,
    pub source: Option<String>,
    pub target: Option<String>,
    pub offset: [f32; 3],
    pub target_offset: [f32; 3],
    pub flatten_y: bool,
}

/// A `Predicate.CanJump` record (`FUN_140792590`): search for a standing place ahead. Distances in cm, relative to the start point
/// (the character plus `start_offset`, turned per `start_option`: 0 by the character, 1 not turned, 2 by the character's yaw, 3 by yaw plus the offset).
#[derive(Debug, Clone, Default)]
pub struct CanJump {
    pub name: String,
    pub start_offset: [f32; 3],
    pub start_option: i32,
    pub distance: (f32, f32),
    pub height: (f32, f32),
    pub width: (f32, f32),
    pub find_furthest: bool,
    pub use_shape: bool,
    /// `WorldYaw` / `YawOffset`: values (radians / degrees) that replace / turn the character's yaw.
    pub world_yaw: Option<String>,
    pub yaw_offset: Option<String>,
    /// The named point the found place is written to, and the values that receive the deltas.
    pub best_position: Option<String>,
    pub out_distance: Option<String>,
    pub out_x: Option<String>,
    pub out_y: Option<String>,
    pub out_z: Option<String>,
    pub out_facing: Option<String>,
}

/// The `Value` names a `LogicNodeData.ContextActionOutput` writes (`FUN_1408ac2c0`).
#[derive(Debug, Clone, Default)]
pub struct ContextOutput {
    pub start_align_x: Option<String>,
    pub start_align_y: Option<String>,
    pub start_align_z: Option<String>,
    pub target_distance: Option<String>,
    pub facing_diff: Option<String>,
    pub facing_normal_diff: Option<String>,
    pub facing_diff_along_line: Option<String>,
    pub world_x: Option<String>,
    pub world_y: Option<String>,
    pub world_z: Option<String>,
    pub world_normal_yaw: Option<String>,
    pub world_normal_along_line_yaw: Option<String>,
    /// `WorldPosition`: the named point the found place is written to.
    pub world_position: Option<String>,
    pub world_space: bool,
    pub query_to_line: bool,
    pub height: f32,
}

#[derive(Debug, Clone)]
pub enum Pred {
    And(Vec<Pred>),
    Or(Vec<Pred>),
    Not(Box<Pred>),
    Range { value: Expr, min: f32, max: f32 },
    ValueChange { value: Expr, threshold: f32 },
    Context(ContextQuery),
    /// `Predicate.Autopicker`: the named auto-picker state has a link that would fire.
    Autopicker(String),
    /// `Predicate.IsSupported`: is there floor within `y_offset` cm (negative: below) of the point `offset` (cm, in the character's frame)?
    Supported { name: String, offset: [f32; 3], y_offset: f32 },
    /// `Predicate.RayCastHit`: a ray (`RayCastData`) that hits the world.
    RayCast(RayCast),
    /// `Predicate.FoundContextActionType`: the last search found a line of this game action (`GameDBContextAction`).
    FoundType(String),
    /// `Predicate.CanJump`: a landing place in a box ahead of the character (`FUN_140792590`).
    CanJump(CanJump),
    /// `Predicate.FoundContextActionBasic`: a line of the type within `radius` and `height` (cm) of the character.
    ContextBasic { action: String, height: f32, radius: f32 },
    ShapeCast { name: String, data: Option<ShapeCast> },
    Command(String),
    Native(String),
    Other(String),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Const(f32),
    /// A value the engine supplies (`RawMoveMag`, `DeltaYaw`, `Game_NodeLifeTime`, ...).
    Named(String),
    If { pred: Box<Pred>, yes: Box<Expr>, no: Box<Expr> },
    Degrees(Box<Expr>),
    /// Seconds the predicate has been true; `id` keys the running count.
    Duration { id: u32, pred: Box<Pred> },
    Dampen { input: Box<Expr>, scale: Box<Expr> },
    Rescale { input: Box<Expr>, from: (f32, f32), to: (f32, f32) },
    Scale { input: Box<Expr>, scale: Box<Expr> },
    Offset { input: Box<Expr>, offset: Box<Expr> },
    Other(String),
}

/// What a state plays: one clip, or a clip blend along a value.
#[derive(Debug, Clone)]
pub enum Anim {
    None,
    Clip(String),
    Blend { value: Expr, points: Vec<(f32, Anim)> },
}

#[derive(Debug, Clone)]
pub struct Transition {
    pub name: String,
    pub target: String,
    pub pred: Option<Pred>,
    /// Fraction of the state's clip before this link may fire.
    pub min: f32,
    pub max: f32,
    pub blend: f32,
    /// `StartPct`: where the entered clip starts (a fraction; negative: the state's own `StartPct`).
    pub start_pct: f32,
    /// `ValueModifiers`: values this link sets when it fires (`Landing_Intensity` 5 ...).
    pub value_mods: Vec<(String, f32)>,
}

#[derive(Debug, Clone)]
pub struct State {
    pub name: String,
    pub anim: Anim,
    pub rate: Option<Expr>,
    pub transitions: Vec<Transition>,
    /// `BlendState` flags: `looping` (the clip repeats), `Gravity` (the state falls under gravity), `IgnoreMovement` (root motion not applied).
    pub looping: bool,
    pub gravity: bool,
    pub ignore_movement: bool,
    /// `DefaultBlendTime`: the cross-fade into the state when the link does not give one.
    pub default_blend: f32,
    /// `StartPct`: where the clip starts when the link gives none.
    pub start_pct: f32,
    /// The `NativeState` type the engine runs the state as (`NativeState.FallingState`, `.SupportedState`, `.TransitionAnimState`).
    pub native: String,
    /// The state's `StateModifier`s: a typed modifier active over a window of the clip.
    pub modifiers: Vec<StateModifier>,
}

/// A `StateModifier`: `kind` (the modifier record's type, e.g. `Modifier.AlignToBrush`) and `name` (e.g. `RailRunAlign`) run from phase `start` to `end`.
#[derive(Debug, Clone)]
pub struct StateModifier {
    pub start: f32,
    pub end: f32,
    pub kind: String,
    pub name: String,
}

impl State {
    /// Is a modifier of this type (and, if given, name) active at `phase`?
    pub fn modifier_active(&self, kind: &str, name: Option<&str>, phase: f32) -> bool {
        self.modifiers.iter().any(|m| m.kind == kind && name.is_none_or(|n| m.name == n) && phase >= m.start && phase <= m.end)
    }
}

/// A state's `Modifier.Amtrak` / `Modifier.AmtrakDFA`: during the clip window `start..end` (fractions of the clip) the root motion is
/// stretched to reach the destination, the playback rate staying within `rate` and the stretch within `scale`.
#[derive(Debug, Clone)]
pub struct AmtrakInfo {
    pub state: String,
    pub record: String,
    pub start: f32,
    pub end: f32,
    pub rate: (f32, f32),
    pub scale: (f32, f32),
    /// `DestWorldPos` (a named point the destination is read from, e.g. `ClimbUseAll_WP`) and `DestOffset` (cm).
    pub dest_world: Option<String>,
    pub dest_offset: [f32; 3],
    /// `DestRot` (`TravelDir`, `Rotation1`, `Rotation2`, ...) and the values `Rotation1` / `Rotation2` name (game yaw, radians).
    pub dest_rot: String,
    /// The modifier goes on into the next state (`EnableExtendToNextState`).
    pub extend: bool,
    pub rotation1: Option<String>,
    pub rotation2: Option<String>,
}

pub struct Graph {
    db: GameDb,
    by_hash: HashMap<u32, (usize, usize)>,
    blend_state: usize,
    /// The plain `Value` records' initial numbers (`Speed_JumpAll` 1.4 ...): what a named value reads before anything sets it.
    defaults: HashMap<String, f32>,
}

struct Parser<'a> {
    g: &'a Graph,
    next_id: u32,
}

impl Graph {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        let db = GameDb::parse(data)?;
        let mut by_hash = HashMap::new();
        let mut blend_state = None;
        for t in 0..db.type_count() {
            let (name, f, r) = db.type_info(t).unwrap();
            if name == "BlendState" {
                blend_state = Some(t);
            }
            for i in f..f + r {
                if let Some(n) = db.item(t, i).and_then(|e| e.name()) {
                    by_hash.entry(name_hash(&n)).or_insert((t, i));
                }
            }
        }
        let mut g = Self { db, by_hash, blend_state: blend_state.context("no BlendState type")?, defaults: HashMap::new() };
        let value_type = (0..g.db.type_count()).find(|&t| g.db.type_info(t).is_some_and(|(n, _, _)| n == "Value"));
        if let Some(t) = value_type {
            let (_, f, r) = g.db.type_info(t).unwrap();
            let mut defaults = HashMap::new();
            for i in f..f + r {
                if let Some(e) = g.db.item(t, i) {
                    if let Some(n) = e.name() {
                        defaults.insert(n, g.float(&e, "Value"));
                    }
                }
            }
            g.defaults = defaults;
        }
        Ok(g)
    }

    /// The initial number of the plain `Value` record `name`.
    pub fn value_default(&self, name: &str) -> Option<f32> {
        self.defaults.get(name).copied()
    }

    fn resolve(&self, v: u32) -> Option<(String, Entry<'_>)> {
        if v == u32::MAX {
            return None;
        }
        if let Some(e) = self.db.follow(v).filter(|e| e.name().is_some()) {
            return Some((self.db.type_info((v >> 20) as usize)?.0, e));
        }
        let &(t, i) = self.by_hash.get(&v)?;
        Some((self.db.type_info(t)?.0, self.db.item(t, i)?))
    }

    fn refs(&self, e: &Entry<'_>, field: &str) -> Vec<u32> {
        match e.field(name_hash(field)) {
            Some(Value::Raw { kind: 10, values }) => values,
            Some(Value::Refs(r)) => r,
            _ => Vec::new(),
        }
    }

    fn float(&self, e: &Entry<'_>, field: &str) -> f32 {
        e.raw_field(name_hash(field)).and_then(|v| v.first()).map_or(0.0, |&b| f32::from_bits(b))
    }

    /// A boolean cell: bit `first` of the record's first value word.
    fn flag(&self, e: &Entry<'_>, field: &str) -> bool {
        let hash = name_hash(field);
        let word = e.raw_values().first().copied().unwrap_or(0);
        e.cells().iter().find(|c| c.hash == hash).is_some_and(|c| (word >> c.first) & 1 == 1)
    }

    /// A vector cell (kind 8): an offset into the float pool (the generic dump shows it as a string; it is three floats).
    fn vec3(&self, e: &Entry<'_>, field: &str) -> [f32; 3] {
        let Some(o) = e.raw_field(name_hash(field)).and_then(|v| v.first().copied()) else { return [0.0; 3] };
        [0, 4, 8].map(|k| self.db.pool_f32(o + k).unwrap_or(0.0))
    }

    /// The name of the `Value` record a reference cell points to, if it is set.
    fn value_ref(&self, e: &Entry<'_>, field: &str) -> Option<String> {
        let v = *self.refs(e, field).first()?;
        self.resolve(v).and_then(|(_, r)| r.name())
    }

    /// The `CollisionData` record a data cell refers to.
    fn collision(&self, d: &Entry<'_>) -> Option<crate::ctxsearch::Collision> {
        let v = *self.refs(d, "CollisionData").first()?;
        let (_, c) = self.resolve(v)?;
        Some(crate::ctxsearch::Collision {
            start_safe: self.float(&c, "StartSafeDist"),
            end_safe: self.float(&c, "EndSafeDist"),
            simple: self.flag(&c, "simple"),
            start_height: self.float(&c, "SimpleStartHeight"),
            end_height: self.float(&c, "SimpleEndHeight"),
            mid_val: self.float(&c, "MidVal"),
            start_offset: self.vec3(&c, "StartOffset"),
            end_offset: self.vec3(&c, "EndOffset"),
            mid_offset: self.vec3(&c, "MidOffset"),
        })
    }

    /// The cells of a `ContextActionData` record that the exe's search reads.
    pub fn context_data(&self, d: &Entry<'_>) -> crate::ctxsearch::Data {
        crate::ctxsearch::Data {
            action: self.text(d, "GameDBContextAction"),
            vertical_range: self.range(d, "VerticalDistRange"),
            vertical_center: match self.text(d, "VerticalDistRangeCenter").as_str() {
                "Center" | "" => 0,
                _ => 255,
            },
            max_horizontal: self.float(d, "MaxHorizontalDist"),
            min_horizontal: self.float(d, "MinHorizontalDist"),
            approach_angle: self.float(d, "ApproachAngle"),
            test_angle_rotation: self.float(d, "TestAngleRotation"),
            approach_test_offset: self.float(d, "ApproachTestOffset"),
            los_range: self.float(d, "LOS_AngleRange"),
            los_left: self.float(d, "LOS_AngleLeft"),
            los_right: self.float(d, "LOS_AngleRight"),
            invert_normal: self.flag(d, "InvertNormal"),
            approach_angle_as_railing: self.flag(d, "ApproachAngleAsRailing"),
            always_pick_center: self.flag(d, "AlwaysPickCenterOfLine"),
            use_distance_from_character: self.flag(d, "UseDistanceFromCharacter"),
            use_distance_from_query: self.flag(d, "UseDistanceFromQuery"),
            use_offset_as_alignment: self.flag(d, "UseOffsetAsAlignment"),
            use_normal_for_found_offset: self.flag(d, "UseNormalForFoundOffset"),
            use_blended_normal_for_found_offset: self.flag(d, "UseBlendedNormalForFoundOffset"),
            distance_test_y_factor: self.float(d, "DistanceTestYFactor"),
            edge_proximity_buffer: self.float(d, "EdgeProximityBuffer"),
            max_connection_dist: self.float(d, "MaxConnectionDist"),
            max_connection_angle: self.float(d, "MaxConnectionAngle"),
            collision: self.collision(d),
            found_node_offset: self.vec3(d, "FoundNodeOffset"),
            additional_node_offset: self.vec3(d, "AdditionalNodeOffset"),
            anim_dims: self.vec3(d, "AnimDims"),
        }
    }

    fn text(&self, e: &Entry<'_>, field: &str) -> String {
        match e.field(name_hash(field)) {
            Some(Value::Strings(s)) => s.into_iter().next().unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn range(&self, e: &Entry<'_>, field: &str) -> (f32, f32) {
        e.raw_field(name_hash(field))
            .and_then(|v| v.first().copied())
            .and_then(|o| Some((self.db.pool_f32(o)?, self.db.pool_f32(o + 4)?)))
            .unwrap_or((f32::MIN, f32::MAX))
    }

    /// The first state that plays `clip` (as a plain animation) and has an Amtrak alignment modifier, with that modifier.
    pub fn amtrak_for_clip(&self, clip: &str) -> Option<AmtrakInfo> {
        let (_, f, r) = self.db.type_info(self.blend_state)?;
        for i in f..f + r {
            let Some(e) = self.db.item(self.blend_state, i) else { continue };
            let plays = self.refs(&e, "AnimationLink").first().and_then(|&v| self.resolve(v)).is_some_and(|(ty, a)| ty == "Animation.SimpleAnimation" && self.text(&a, "animation").eq_ignore_ascii_case(clip));
            if !plays {
                continue;
            }
            if let Some(a) = self.amtrak_of(&e) {
                return Some(a);
            }
        }
        None
    }

    /// The Amtrak alignment of the state `name` (its `Modifiers`), whatever clip tree it plays.
    pub fn amtrak_for_state(&self, name: &str) -> Option<AmtrakInfo> {
        let (_, f, r) = self.db.type_info(self.blend_state)?;
        let e = (f..f + r).filter_map(|i| self.db.item(self.blend_state, i)).find(|e| e.name().as_deref() == Some(name))?;
        self.amtrak_of(&e)
    }

    fn amtrak_of(&self, e: &Entry<'_>) -> Option<AmtrakInfo> {
        for m in self.refs(e, "Modifiers") {
            let Some((_, sm)) = self.resolve(m) else { continue };
            let Some((ty, a)) = self.refs(&sm, "Modifier").first().and_then(|&v| self.resolve(v)) else { continue };
            if !ty.starts_with("Modifier.Amtrak") {
                continue;
            }
            return Some(AmtrakInfo {
                state: e.name().unwrap_or_default(),
                record: a.name().unwrap_or_default(),
                start: self.float(&sm, "Start"),
                end: self.float(&sm, "End"),
                rate: self.range(&a, "RateMinMax"),
                scale: self.range(&a, "ScaleMinMax"),
                dest_world: self.value_ref(&a, "DestWorldPos"),
                dest_offset: self.vec3(&a, "DestOffset"),
                dest_rot: self.text(&a, "DestRot"),
                extend: self.flag(&sm, "EnableExtendToNextState"),
                rotation1: self.value_ref(&a, "ÂRotation1").or_else(|| self.value_ref(&a, "Rotation1")),
                rotation2: self.value_ref(&a, "Rotation2"),
            });
        }
        None
    }

    /// Every state's name.
    pub fn state_names(&self) -> Vec<String> {
        let Some((_, f, r)) = self.db.type_info(self.blend_state) else { return Vec::new() };
        (f..f + r).filter_map(|i| self.db.item(self.blend_state, i)).filter_map(|e| e.name()).collect()
    }

    /// A transition's `ValueModifiers`: the `Value` each sets and the number (the record's own number, else its name read as one).
    fn value_mods(&self, t: &Entry<'_>) -> Vec<(String, f32)> {
        let Some(Value::Refs(list)) = t.field(name_hash("ValueModifiers")) else { return Vec::new() };
        list.into_iter()
            .filter_map(|r| {
                let m = self.db.follow(r)?;
                let node = self.value_ref(&m, "ValueNode")?;
                let (_, n) = self.refs(&m, "Number").first().and_then(|&v| self.resolve(v))?;
                let number = n.name().and_then(|s| s.parse::<f32>().ok()).unwrap_or_else(|| self.float(&n, "Value"));
                Some((node, number))
            })
            .collect()
    }

    /// The state `name`, with its clip tree and transitions parsed.
    pub fn state(&self, name: &str) -> Option<State> {
        let (_, f, r) = self.db.type_info(self.blend_state)?;
        let e = (f..f + r).filter_map(|i| self.db.item(self.blend_state, i)).find(|e| e.name().as_deref() == Some(name))?;
        let mut p = Parser { g: self, next_id: 0 };
        let anim = self.refs(&e, "AnimationLink").first().map_or(Anim::None, |&v| p.anim(v, 0));
        let rate = self.refs(&e, "Rate").first().filter(|&&v| v != u32::MAX).map(|&v| p.expr(v, 0));
        let mut transitions = Vec::new();
        for link in self.refs(&e, "Links") {
            let Some((_, t)) = self.resolve(link) else { continue };
            let target = self.refs(&t, "node").first().and_then(|&v| self.resolve(v)).and_then(|(_, e)| e.name()).unwrap_or_default();
            let pred = self.refs(&t, "Predicate").first().filter(|&&v| v != u32::MAX).map(|&v| p.pred(v, 0));
            transitions.push(Transition {
                name: t.name().unwrap_or_default(),
                target,
                pred,
                min: self.float(&t, "MinTransition"),
                max: self.float(&t, "MaxTransition"),
                blend: self.float(&t, "blendtime"),
                start_pct: self.float(&t, "StartPct"),
                value_mods: self.value_mods(&t),
            });
        }
        Some(State {
            name: name.to_string(),
            anim,
            rate,
            transitions,
            looping: self.flag(&e, "looping"),
            gravity: self.flag(&e, "Gravity"),
            ignore_movement: self.flag(&e, "IgnoreMovement"),
            default_blend: self.float(&e, "DefaultBlendTime"),
            start_pct: self.float(&e, "StartPct"),
            native: self.refs(&e, "NativeState").first().and_then(|&v| self.resolve(v)).map(|(t, _)| t).unwrap_or_default(),
            modifiers: self
                .refs(&e, "Modifiers")
                .into_iter()
                .filter_map(|v| {
                    let (_, m) = self.resolve(v)?;
                    let (kind, target) = self.refs(&m, "Modifier").first().and_then(|&x| self.resolve(x))?;
                    Some(StateModifier { start: self.float(&m, "Start"), end: self.float(&m, "End"), kind, name: target.name().unwrap_or_default() })
                })
                .collect(),
        })
    }
}

impl Parser<'_> {
    fn anim(&mut self, v: u32, depth: usize) -> Anim {
        let Some((ty, e)) = self.g.resolve(v) else { return Anim::None };
        match ty.as_str() {
            "Animation.SimpleAnimation" => Anim::Clip(self.g.text(&e, "animation")),
            "SimplePAnimNode" if depth < 6 => {
                let value = self.g.refs(&e, "Value").first().map_or(Expr::Const(0.0), |&x| self.expr(x, 0));
                let mut points = Vec::new();
                if let Some(Value::Refs(list)) = e.field(name_hash("AnimPoint")) {
                    for r in list {
                        let Some(point) = self.g.db.follow(r) else { continue };
                        let at = self.g.float(&point, "point");
                        let sub = self.g.refs(&point, "Anim").first().map_or(Anim::None, |&x| self.anim(x, depth + 1));
                        points.push((at, sub));
                    }
                }
                Anim::Blend { value, points }
            }
            _ => Anim::None,
        }
    }

    fn pred(&mut self, v: u32, depth: usize) -> Pred {
        let Some((ty, e)) = self.g.resolve(v) else { return Pred::Other("?".into()) };
        let name = e.name().unwrap_or_default();
        if depth > 14 {
            return Pred::Other(format!("{ty}:{name} (too deep)"));
        }
        let list = |s: &mut Self, field: &str| -> Vec<Pred> { s.g.refs(&e, field).into_iter().map(|x| s.pred(x, depth + 1)).collect() };
        match ty.as_str() {
            "Predicate.And" => Pred::And(list(self, "PREDICATES")),
            "Predicate.Or" => Pred::Or(list(self, "PREDICATES")),
            "Predicate.Not" => Pred::Not(Box::new(self.g.refs(&e, "Predicate").first().map_or(Pred::Other("empty not".into()), |&x| self.pred(x, depth + 1)))),
            "Predicate.Range" => {
                let value = self.g.refs(&e, "Value").first().map_or(Expr::Const(0.0), |&x| self.expr(x, depth + 1));
                let (min, max) = self.g.range(&e, "range");
                Pred::Range { value, min, max }
            }
            "Predicate.ValueChange" => {
                let value = self.g.refs(&e, "Value").first().map_or(Expr::Const(0.0), |&x| self.expr(x, depth + 1));
                Pred::ValueChange { value, threshold: self.g.float(&e, "Threshold") }
            }
            "Predicate.FoundContextAction" => {
                let q = self.g.refs(&e, "ContextActionData").first().and_then(|&x| self.g.resolve(x));
                match q {
                    Some((_, d)) => Pred::Context(ContextQuery {
                        name: d.name().unwrap_or_default(),
                        action: self.g.text(&d, "GameDBContextAction"),
                        max_dist: self.g.float(&d, "MaxHorizontalDist"),
                        min_dist: self.g.float(&d, "MinHorizontalDist"),
                        approach_angle: self.g.float(&d, "ApproachAngle"),
                        test_angle_rotation: self.g.float(&d, "TestAngleRotation"),
                        los_angle: self.g.float(&d, "LOS_AngleRange"),
                        max_connection_dist: self.g.float(&d, "MaxConnectionDist"),
                        max_connection_angle: self.g.float(&d, "MaxConnectionAngle"),
                        data: self.g.context_data(&d),
                        test_override: self.g.value_ref(&d, "TestAngleRotationOverride"),
                        world_override: self.g.value_ref(&d, "TestAngleRotationWorldOverride"),
                        query_position: self.g.value_ref(&d, "QueryWorldPosition"),
                        output: self.g.refs(&e, "ContextActionOutput").first().and_then(|&x| self.g.resolve(x)).map(|(_, o)| ContextOutput {
                            start_align_x: self.g.value_ref(&o, "StartAlignX"),
                            start_align_y: self.g.value_ref(&o, "StartAlignY"),
                            start_align_z: self.g.value_ref(&o, "StartAlignZ"),
                            target_distance: self.g.value_ref(&o, "TargetDistance"),
                            facing_diff: self.g.value_ref(&o, "FacingDiff"),
                            facing_normal_diff: self.g.value_ref(&o, "FacingNormalDiff"),
                            facing_diff_along_line: self.g.value_ref(&o, "FacingDiffAlongLine"),
                            world_x: self.g.value_ref(&o, "WorldX"),
                            world_y: self.g.value_ref(&o, "WorldY"),
                            world_z: self.g.value_ref(&o, "WorldZ"),
                            world_normal_yaw: self.g.value_ref(&o, "WorldNormalYaw"),
                            world_normal_along_line_yaw: self.g.value_ref(&o, "WorldNormalAlongLineYaw"),
                            world_position: self.g.value_ref(&o, "WorldPosition"),
                            world_space: self.g.flag(&o, "AlignmentInWorldSpace"),
                            query_to_line: self.g.text(&o, "FacingDiffAlignment") == "QueryToLine",
                            height: self.g.float(&d, "Height"),
                        }),
                    }),
                    None => Pred::Other(format!("{ty}:{name} (no data)")),
                }
            }
            "Predicate.FoundContextActionBasic" => Pred::ContextBasic {
                action: self.g.text(&e, "GameDBContextAction"),
                height: self.g.float(&e, "QueryHeight"),
                radius: self.g.float(&e, "QueryRadius"),
            },
            "Predicate.ShapeCastHit" => {
                let data = self.g.refs(&e, "shapecast").first().and_then(|&x| self.g.resolve(x)).map(|(_, c)| ShapeCast {
                    radius: self.g.float(&c, "ShapeRadius"),
                    height: self.g.float(&c, "ShapeHeight"),
                    offset: self.g.vec3(&c, "offset"),
                    target_offset: self.g.vec3(&c, "TargetOffset"),
                    flatten_y: self.g.flag(&c, "DirectionFlattenY"),
                    local: self.g.flag(&c, "local"),
                    directional: self.g.flag(&c, "TargetOffsetInDirectionalSpace"),
                    target_world: self.g.value_ref(&c, "TargetWorldPosition"),
                    target_x: self.g.value_ref(&c, "TargetX"),
                    target_y: self.g.value_ref(&c, "TargetY"),
                    target_z: self.g.value_ref(&c, "TargetZ"),
                });
                Pred::ShapeCast { name, data }
            }
            "Predicate.Autopicker" => match self.g.refs(&e, "AutopickerNode").first().and_then(|&x| self.g.resolve(x)).and_then(|(_, n)| n.name()) {
                Some(state) => Pred::Autopicker(state),
                None => Pred::Other(format!("{ty}:{name} (no node)")),
            },
            "Predicate.CanJump" => Pred::CanJump(CanJump {
                name,
                start_offset: self.g.vec3(&e, "StartingOffset"),
                start_option: e.raw_field(name_hash("StartingOffsetOption")).and_then(|v| v.first().copied()).unwrap_or(0) as i32,
                distance: self.g.range(&e, "DistanceRange"),
                height: self.g.range(&e, "HeightRange"),
                width: self.g.range(&e, "WidthRange"),
                find_furthest: self.g.flag(&e, "FindFurthest"),
                use_shape: self.g.flag(&e, "UseCanJumpShape"),
                world_yaw: self.g.value_ref(&e, "WorldYaw"),
                yaw_offset: self.g.value_ref(&e, "YawOffset"),
                best_position: self.g.value_ref(&e, "BestFoundPosition"),
                out_distance: self.g.value_ref(&e, "OutputDistance"),
                out_x: self.g.value_ref(&e, "OutputXDelta"),
                out_y: self.g.value_ref(&e, "OutputYDelta"),
                out_z: self.g.value_ref(&e, "OutputZDelta"),
                out_facing: self.g.value_ref(&e, "OutputFacingDiff"),
            }),
            "Predicate.IsSupported" => {
                let d = self.g.refs(&e, "SupportedData").first().and_then(|&x| self.g.db.follow(x));
                match d {
                    Some(d) => Pred::Supported { name, offset: self.g.vec3(&d, "StartOffset"), y_offset: self.g.float(&d, "YOffset") },
                    None => Pred::Other(format!("{ty}:{name} (no data)")),
                }
            }
            "Predicate.RayCastHit" => match self.g.refs(&e, "Raycast").first().and_then(|&x| self.g.resolve(x)) {
                Some((_, r)) => Pred::RayCast(RayCast {
                    name,
                    source: self.g.value_ref(&r, "SourceWorldPosition"),
                    target: self.g.value_ref(&r, "TargetWorldPosition"),
                    offset: self.g.vec3(&r, "offset"),
                    target_offset: self.g.vec3(&r, "TargetOffset"),
                    flatten_y: self.g.flag(&r, "DirectionFlattenY"),
                }),
                None => Pred::Other(format!("{ty}:{name} (no data)")),
            },
            "Predicate.FoundContextActionType" => Pred::FoundType(self.g.text(&e, "GameDBContextAction")),
            "Predicate.Command" => Pred::Command(self.g.text(&e, "ActionId")),
            "Predicate.Native" => Pred::Native(name),
            _ => Pred::Other(format!("{ty}:{name}")),
        }
    }

    fn expr(&mut self, v: u32, depth: usize) -> Expr {
        let Some((ty, e)) = self.g.resolve(v) else { return Expr::Const(0.0) };
        let name = e.name().unwrap_or_default();
        if depth > 14 {
            return Expr::Other(format!("{ty}:{name} (too deep)"));
        }
        let sub = |s: &mut Self, field: &str| -> Expr { s.g.refs(&e, field).first().map_or(Expr::Const(0.0), |&x| s.expr(x, depth + 1)) };
        match ty.as_str() {
            "Value" => match name.parse::<f32>() {
                Ok(c) => Expr::Const(c),
                Err(_) => Expr::Named(name),
            },
            "Value.If" => {
                let pred = self.g.refs(&e, "Predicate").first().map_or(Pred::Other("no predicate".into()), |&x| self.pred(x, depth + 1));
                Expr::If { pred: Box::new(pred), yes: Box::new(sub(self, "true")), no: Box::new(sub(self, "FALSE")) }
            }
            "Value.ToDegrees" => Expr::Degrees(Box::new(sub(self, "input"))),
            "Value.Duration" => {
                let pred = self.g.refs(&e, "Predicate").first().map_or(Pred::Other("no predicate".into()), |&x| self.pred(x, depth + 1));
                // The counter belongs to the record, not to the state that happens to use it.
                Expr::Duration { id: name_hash(&name), pred: Box::new(pred) }
            }
            "Value.Dampener" => Expr::Dampen { input: Box::new(sub(self, "input")), scale: Box::new(sub(self, "scale")) },
            "Value.Rescale" => Expr::Rescale {
                input: Box::new(sub(self, "input")),
                from: self.g.range(&e, "from"),
                // The destination range is stored under a field the exe names nothing for (`000047db`).
                to: e.raw_field(0x47db).and_then(|v| v.first().copied()).and_then(|o| Some((self.g.db.pool_f32(o)?, self.g.db.pool_f32(o + 4)?))).unwrap_or((0.0, 1.0)),
            },
            "Value.Scaler" => Expr::Scale { input: Box::new(sub(self, "input")), scale: Box::new(sub(self, "scale")) },
            "Value.Offset" => Expr::Offset { input: Box::new(sub(self, "input")), offset: Box::new(sub(self, "offset")) },
            _ => Expr::Other(format!("{ty}:{name}")),
        }
    }
}

/// What the evaluator asks of the game world.
pub trait Env {
    /// An engine value by name.
    fn named(&mut self, name: &str) -> f32;
    /// Whether the input `action` (a `Predicate.Command` `ActionId`) is held.
    fn command(&mut self, action: &str) -> bool;
    fn native(&mut self, name: &str) -> bool;
    fn context(&mut self, q: &ContextQuery) -> bool;
    fn context_basic(&mut self, action: &str, height: f32, radius: f32) -> bool;
    fn shape_cast(&mut self, name: &str, data: Option<&ShapeCast>) -> bool;
    fn autopicker(&mut self, state: &str) -> bool;
    fn can_jump(&mut self, c: &CanJump) -> bool;
    fn supported(&mut self, offset: [f32; 3], y_offset: f32) -> bool;
    fn ray_cast(&mut self, r: &RayCast) -> bool;
    fn found_type(&mut self, action: &str) -> bool;
}

/// Running state of the `Value.Duration` counters and the previous values `ValueChange` compares with.
#[derive(Default, Clone)]
pub struct Memo {
    durations: HashMap<u32, f32>,
    /// Names of predicates and values the evaluator could not interpret (each reported once).
    pub unknown: Vec<String>,
}

impl Memo {
    fn note(&mut self, what: String) {
        if !self.unknown.contains(&what) {
            self.unknown.push(what);
        }
    }
}

pub fn eval_pred(p: &Pred, env: &mut dyn Env, memo: &mut Memo) -> bool {
    match p {
        Pred::And(list) => list.iter().all(|x| eval_pred(x, env, memo)),
        Pred::Or(list) => list.iter().any(|x| eval_pred(x, env, memo)),
        Pred::Not(x) => !eval_pred(x, env, memo),
        Pred::Range { value, min, max } => {
            let v = eval_expr(value, env, memo);
            // A range written high-to-low (`99999 .. -56`) is the same interval.
            let (lo, hi) = if min <= max { (*min, *max) } else { (*max, *min) };
            v >= lo && v <= hi
        }
        Pred::ValueChange { value, threshold } => eval_expr(value, env, memo).abs() > *threshold,
        Pred::Context(q) => env.context(q),
        Pred::ContextBasic { action, height, radius } => env.context_basic(action, *height, *radius),
        Pred::ShapeCast { name, data } => env.shape_cast(name, data.as_ref()),
        Pred::Command(a) => env.command(a),
        Pred::Native(n) => env.native(n),
        Pred::Autopicker(n) => env.autopicker(n),
        Pred::CanJump(c) => env.can_jump(c),
        Pred::Supported { offset, y_offset, .. } => env.supported(*offset, *y_offset),
        Pred::RayCast(r) => env.ray_cast(r),
        Pred::FoundType(a) => env.found_type(a),
        Pred::Other(n) => {
            memo.note(n.clone());
            false
        }
    }
}

/// `eval_pred` that also records every leaf's result, indented by depth (for debugging a link that does not fire).
pub fn trace_pred(p: &Pred, env: &mut dyn Env, memo: &mut Memo, depth: usize, out: &mut Vec<String>) -> bool {
    let pad = "  ".repeat(depth);
    match p {
        Pred::And(v) => {
            out.push(format!("{pad}And"));
            let mut all = true;
            for c in v {
                all &= trace_pred(c, env, memo, depth + 1, out);
            }
            all
        }
        Pred::Or(v) => {
            out.push(format!("{pad}Or"));
            let mut any = false;
            for c in v {
                any |= trace_pred(c, env, memo, depth + 1, out);
            }
            any
        }
        Pred::Not(c) => {
            out.push(format!("{pad}Not"));
            !trace_pred(c, env, memo, depth + 1, out)
        }
        leaf => {
            let ok = eval_pred(leaf, env, memo);
            let label = match leaf {
                Pred::Range { value, min, max } => format!("Range {} in {min}..{max} (= {:.2})", expr_label(value), eval_expr(value, env, memo)),
                Pred::Context(q) => format!("Context {} [{}]", q.name, q.action),
                Pred::ContextBasic { action, .. } => format!("ContextBasic {action}"),
                Pred::ShapeCast { name, .. } => format!("ShapeCast {name}"),
                Pred::CanJump(c) => format!("CanJump {}", c.name),
                Pred::Command(a) => format!("Command {a}"),
                Pred::Native(n) => format!("Native {n}"),
                Pred::Autopicker(n) => format!("Autopicker {n}"),
                Pred::Other(n) => format!("Other {n}"),
                _ => "?".into(),
            };
            out.push(format!("{pad}{label} = {ok}"));
            ok
        }
    }
}

fn expr_label(e: &Expr) -> String {
    match e {
        Expr::Named(n) => n.clone(),
        Expr::Const(c) => c.to_string(),
        Expr::Degrees(x) => format!("deg({})", expr_label(x)),
        Expr::If { .. } => "if(..)".into(),
        Expr::Duration { .. } => "duration".into(),
        _ => "expr".into(),
    }
}

pub fn eval_expr(e: &Expr, env: &mut dyn Env, memo: &mut Memo) -> f32 {
    match e {
        Expr::Const(c) => *c,
        Expr::Named(n) => env.named(n),
        Expr::If { pred, yes, no } => {
            if eval_pred(pred, env, memo) { eval_expr(yes, env, memo) } else { eval_expr(no, env, memo) }
        }
        Expr::Degrees(x) => eval_expr(x, env, memo).to_degrees(),
        Expr::Duration { id, .. } => memo.durations.get(id).copied().unwrap_or(0.0),
        // The dampener's smoothing is the animation cross-fade the caller does; the target value is what matters here.
        Expr::Dampen { input, .. } => eval_expr(input, env, memo),
        Expr::Rescale { input, from, to } => {
            let t = if from.1 != from.0 { ((eval_expr(input, env, memo) - from.0) / (from.1 - from.0)).clamp(0.0, 1.0) } else { 0.0 };
            to.0 + (to.1 - to.0) * t
        }
        Expr::Scale { input, scale } => eval_expr(input, env, memo) * eval_expr(scale, env, memo),
        Expr::Offset { input, offset } => eval_expr(input, env, memo) + eval_expr(offset, env, memo),
        Expr::Other(n) => {
            memo.note(n.clone());
            0.0
        }
    }
}

/// Advance every `Value.Duration` reachable from `p` by `dt`: a counter runs while its predicate holds, and resets when not.
pub fn tick_durations(p: &Pred, env: &mut dyn Env, memo: &mut Memo, dt: f32) {
    fn walk_expr(e: &Expr, env: &mut dyn Env, memo: &mut Memo, dt: f32) {
        match e {
            Expr::Duration { id, pred } => {
                tick_durations(pred, env, memo, dt);
                let on = eval_pred(pred, env, memo);
                let c = memo.durations.entry(*id).or_insert(0.0);
                *c = if on { *c + dt } else { 0.0 };
            }
            Expr::If { pred, yes, no } => {
                tick_durations(pred, env, memo, dt);
                walk_expr(yes, env, memo, dt);
                walk_expr(no, env, memo, dt);
            }
            Expr::Degrees(x) => walk_expr(x, env, memo, dt),
            Expr::Dampen { input, scale } | Expr::Scale { input, scale } => {
                walk_expr(input, env, memo, dt);
                walk_expr(scale, env, memo, dt);
            }
            Expr::Rescale { input, .. } => walk_expr(input, env, memo, dt),
            Expr::Offset { input, offset } => {
                walk_expr(input, env, memo, dt);
                walk_expr(offset, env, memo, dt);
            }
            _ => {}
        }
    }
    match p {
        Pred::And(l) | Pred::Or(l) => l.iter().for_each(|x| tick_durations(x, env, memo, dt)),
        Pred::Not(x) => tick_durations(x, env, memo, dt),
        Pred::Range { value, .. } | Pred::ValueChange { value, .. } => walk_expr(value, env, memo, dt),
        _ => {}
    }
}

/// The numbers a `CameraBlendState`'s modifier stack sets for the player camera (distances in cm, angles in degrees, times in seconds).
/// Each number is the state's `CameraUpdateValues` setting for the modifier's named `Value` if there is one, else that `Value`'s
/// initial number, else the constant on the modifier. Conditional settings and the modifiers' predicates are not evaluated.
#[derive(Debug, Clone, Default)]
pub struct CameraProfile {
    pub name: String,
    /// `CameraSetRoot`: the bone the camera follows and its smoothing times (x, y, z) for the position and the offset.
    pub focus: String,
    pub pos_smooth: [f32; 3],
    pub offset_smooth: [f32; 3],
    /// `CameraDistance`: orbit distance limits.
    pub dist_min: f32,
    pub dist_max: f32,
    /// `CameraPitch`: the resting pitch, how fast it returns (`DefaultSmoothTime`), the limits and the top/mid/bottom orbit offsets of `CameraOrbitControl`.
    pub pitch_default: f32,
    pub pitch_smooth: f32,
    pub pitch_limits: (f32, f32),
    pub mid_offset: [f32; 3],
    /// `CameraYaw`: the smoothing time, the top speed (deg/s), the speed (cm/s) and the angle (deg) that start it.
    pub yaw_smooth: f32,
    pub yaw_max_speed: f32,
    pub yaw_velocity: f32,
    pub yaw_thresh: f32,
    /// `CameraLens`: the field of view (degrees).
    pub fov: f32,
}

impl Graph {
    /// The camera numbers of the `CameraBlendState` `name`.
    pub fn camera_profile(&self, name: &str) -> Option<CameraProfile> {
        let t = (0..self.db.type_count()).find(|&t| self.db.type_info(t).is_some_and(|(n, _, _)| n == "CameraBlendState"))?;
        let (_, f, r) = self.db.type_info(t)?;
        let e = (f..f + r).filter_map(|i| self.db.item(t, i)).find(|e| e.name().as_deref() == Some(name))?;
        let mods: Vec<(String, Entry<'_>)> = self.refs(&e, "CameraModifiers").into_iter().filter_map(|v| self.resolve(v)).collect();
        // The state's own value settings.
        let mut settings: HashMap<String, f32> = HashMap::new();
        for (ty, m) in &mods {
            if ty != "CameraModifier.CameraUpdateValues" {
                continue;
            }
            if let Some(Value::Refs(list)) = m.field(name_hash("DefaultValues")) {
                for r in list {
                    let Some(s) = self.db.follow(r) else { continue };
                    if let Some(n) = self.value_ref(&s, "ValueNode") {
                        settings.insert(n, self.float(&s, "Value"));
                    }
                }
            }
        }
        let num = |e: &Entry<'_>, field: &str, value_field: &str| -> f32 {
            match self.value_ref(e, value_field) {
                Some(n) => settings.get(&n).copied().or_else(|| self.value_default(&n)).unwrap_or_else(|| self.float(e, field)),
                None => self.float(e, field),
            }
        };
        let mut p = CameraProfile { name: name.to_string(), fov: 50.0, ..Default::default() };
        for (ty, m) in &mods {
            match ty.as_str() {
                "CameraModifier.CameraSetRoot" => {
                    p.focus = self.value_ref(m, "FocusPoint").unwrap_or_default();
                    p.pos_smooth = self.vec3(m, "PosSmoothTime");
                    p.offset_smooth = self.vec3(m, "OffsetSmoothTime");
                }
                "CameraModifier.CameraDistance" => {
                    p.dist_min = num(m, "MinDistance", "MinDistanceValue");
                    p.dist_max = num(m, "MaxDistance", "MaxDistanceValue");
                }
                "CameraModifier.CameraPitch" => {
                    p.pitch_default = num(m, "DefaultPitch", "DefaultPitchValue");
                    p.pitch_smooth = num(m, "DefaultSmoothTime", "DefaultSmoothTimeValue");
                    p.pitch_limits = self.range(m, "PitchLimits");
                }
                "CameraModifier.CameraOrbitControl" => {
                    p.mid_offset = self.vec3(m, "MidOffset");
                }
                "CameraModifier.CameraYaw" => {
                    p.yaw_smooth = num(m, "SmoothTime", "SmoothTimeValue");
                    p.yaw_max_speed = num(m, "MaxSpeed", "MaxSpeedValue");
                    p.yaw_velocity = num(m, "VelocityThreshold", "VelocityThresholdValue");
                    p.yaw_thresh = num(m, "YawThresh", "YawThreshValue");
                }
                "CameraModifier.CameraLens" => {
                    if let Some((_, lens)) = self.refs(m, "CameraLensData").first().and_then(|&v| self.resolve(v)) {
                        p.fov = num(&lens, "fov", "FovValue");
                    }
                }
                _ => {}
            }
        }
        Some(p)
    }
}

/// A `Modifier.Pinn` record: hold a hand or foot where it is (or was put) with IK. Times in seconds, distances in cm.
#[derive(Debug, Clone, Default)]
pub struct PinnData {
    pub name: String,
    /// `type` ("Hand" / "Foot") and `part` ("Left" / "Right").
    pub kind: String,
    pub part: String,
    pub max_factor: f32,
    pub min_factor: f32,
    pub release_time: f32,
    /// `ReleaseDist`: the pin lets go when the animated limb is this far from it.
    pub release_dist: (f32, f32),
}

/// A `Modifier.FindMoveAndPinIK` record: search for a place (a context line from the limb, or a ray) and put the limb there.
#[derive(Debug, Clone, Default)]
pub struct FindPinData {
    pub name: String,
    pub kind: String,
    pub part: String,
    pub max_factor: f32,
    pub min_factor: f32,
    pub blend_out: f32,
    /// `SearchOriginOverride`: a named point the search starts from instead of the limb.
    pub search_origin: Option<String>,
    pub context: Option<crate::ctxsearch::Data>,
    pub raycast: Option<RayCast>,
}

impl Graph {
    fn record(&self, ty: &str, name: &str) -> Option<Entry<'_>> {
        let t = (0..self.db.type_count()).find(|&t| self.db.type_info(t).is_some_and(|(n, _, _)| n == ty))?;
        let (_, f, r) = self.db.type_info(t)?;
        (f..f + r).filter_map(|i| self.db.item(t, i)).find(|e| e.name().as_deref() == Some(name))
    }

    pub fn pinn(&self, name: &str) -> Option<PinnData> {
        let e = self.record("Modifier.Pinn", name)?;
        Some(PinnData {
            name: name.to_string(),
            kind: self.text(&e, "type"),
            part: self.text(&e, "part"),
            max_factor: self.float(&e, "MaxFactor"),
            min_factor: self.float(&e, "MinFactor"),
            release_time: self.float(&e, "ReleaseTime"),
            release_dist: self.range(&e, "ReleaseDist"),
        })
    }

    pub fn find_pin(&self, name: &str) -> Option<FindPinData> {
        let e = self.record("Modifier.FindMoveAndPinIK", name)?;
        let context = self.refs(&e, "ContextActionData").first().and_then(|&v| self.resolve(v)).map(|(_, d)| self.context_data(&d));
        let raycast = self.refs(&e, "Raycast").first().and_then(|&v| self.resolve(v)).map(|(_, r)| RayCast {
            name: r.name().unwrap_or_default(),
            source: self.value_ref(&r, "SourceWorldPosition"),
            target: self.value_ref(&r, "TargetWorldPosition"),
            offset: self.vec3(&r, "offset"),
            target_offset: self.vec3(&r, "TargetOffset"),
            flatten_y: self.flag(&r, "DirectionFlattenY"),
        });
        Some(FindPinData {
            name: name.to_string(),
            kind: self.text(&e, "type"),
            part: self.text(&e, "part"),
            max_factor: self.float(&e, "MaxFactor"),
            min_factor: self.float(&e, "MinFactor"),
            blend_out: self.float(&e, "BlendOut"),
            search_origin: self.value_ref(&e, "SearchOriginOverride"),
            context,
            raycast,
        })
    }
}
