//! The player behaviour graph (`behaviors/player/player_elf.bvr`), read for the ground gaits and the camera: which clip and playback rate
//! `Walking`, `Sprint`, `Walking_To_Idle`, `RunToIdle_*` play (their clip trees and `Rate` expressions), and the camera profiles.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use som_formats::animator::{ClipId, Library};
use som_formats::bvrgraph::{Anim, CameraProfile, CanJump, ContextQuery, Env, Graph, Memo, RayCast, ShapeCast, State, eval_expr};
use som_formats::vfs::Vfs;

pub struct GaitGraph {
    graph: Graph,
    states: Mutex<HashMap<String, Option<Arc<State>>>>,
    cameras: Mutex<HashMap<String, Option<CameraProfile>>>,
}

/// What the gait clip trees and rates ask of the player.
#[derive(Clone, Copy, Default)]
pub struct GroundCtx {
    /// `Native("Stealth")` / `CanHide`: crouched.
    pub stealth: bool,
    /// `Native("InCombat")`: the sword is out (my reading; the exe's rule is not decoded).
    pub in_combat: bool,
    /// `Game_NodeLifeTime`: seconds in the state.
    pub node_time: f32,
    /// `RawMoveMag`: stick magnitude 0..1.
    pub move_mag: f32,
}

struct GroundEnv<'a> {
    graph: &'a GaitGraph,
    ctx: GroundCtx,
}

impl Env for GroundEnv<'_> {
    fn named(&mut self, name: &str) -> f32 {
        match name {
            "RawMoveMag" => self.ctx.move_mag,
            "Game_NodeLifeTime" => self.ctx.node_time,
            "Game_GroundPitch" | "Am_I_Sliding" => 0.0,
            _ => self.graph.graph.value_default(name).unwrap_or(0.0),
        }
    }
    fn command(&mut self, _: &str) -> bool {
        false
    }
    fn native(&mut self, name: &str) -> bool {
        match name {
            "Stealth" | "CanHide" => self.ctx.stealth,
            "InCombat" => self.ctx.in_combat,
            _ => false,
        }
    }
    fn context(&mut self, _: &ContextQuery) -> bool {
        false
    }
    fn context_basic(&mut self, _: &str, _: f32, _: f32) -> bool {
        false
    }
    fn shape_cast(&mut self, _: &str, _: Option<&ShapeCast>) -> bool {
        false
    }
    fn autopicker(&mut self, _: &str) -> bool {
        false
    }
    fn can_jump(&mut self, _: &CanJump) -> bool {
        false
    }
    fn supported(&mut self, _: [f32; 3], _: f32) -> bool {
        false
    }
    fn ray_cast(&mut self, _: &RayCast) -> bool {
        false
    }
    fn found_type(&mut self, _: &str) -> bool {
        false
    }
}

impl GaitGraph {
    pub fn load() -> anyhow::Result<Arc<Self>> {
        let mut vfs = Vfs::open(Vfs::locate()?)?;
        Ok(Arc::new(Self { graph: Graph::parse(vfs.read("behaviors/player/player_elf.bvr")?)?, states: Mutex::new(HashMap::new()), cameras: Mutex::new(HashMap::new()) }))
    }

    fn state(&self, name: &str) -> Option<Arc<State>> {
        let mut c = self.states.lock().ok()?;
        c.entry(name.to_string()).or_insert_with(|| self.graph.state(name).map(Arc::new)).clone()
    }

    /// The numbers of a camera state (`CameraBlendState`), read once.
    pub fn camera(&self, name: &str) -> Option<CameraProfile> {
        let mut c = self.cameras.lock().ok()?;
        c.entry(name.to_string()).or_insert_with(|| self.graph.camera_profile(name)).clone()
    }

    /// The clip and playback rate a ground state plays under `ctx`: the clip tree walked along its blend values to the nearest point
    /// (the cross-fade between neighbouring points is the animator's), the state's `Rate` expression for the speed.
    pub fn steady(&self, state: &str, ctx: GroundCtx, lib: &Library) -> Option<(ClipId, f32)> {
        let st = self.state(state)?;
        let mut env = GroundEnv { graph: self, ctx };
        let mut memo = Memo::default();
        fn walk(a: &Anim, env: &mut GroundEnv<'_>, memo: &mut Memo, lib: &Library) -> Option<ClipId> {
            match a {
                Anim::None => None,
                Anim::Clip(n) => lib.find(n),
                Anim::Blend { value, points } => {
                    let v = eval_expr(value, env, memo);
                    let best = points.iter().min_by(|a, b| (a.0 - v).abs().total_cmp(&(b.0 - v).abs()))?;
                    walk(&best.1, env, memo, lib)
                }
            }
        }
        let clip = walk(&st.anim, &mut env, &mut memo, lib)?;
        let rate = st.rate.as_ref().map_or(1.0, |e| eval_expr(e, &mut env, &mut memo)).max(0.05);
        Some((clip, rate))
    }
}
