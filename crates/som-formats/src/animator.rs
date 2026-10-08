//! Playback, cross-fading and root motion on top of [`crate::anix`].
//!
//! Banks hold parent-relative transforms for their own node subset; a [`Library`] maps every bank
//! onto the skeleton (bones a bank does not animate stay in their bind pose) so clips from
//! different banks can be blended.
//!
//! The skeleton's `Null` bone carries the character's travel. [`Animator::update`] strips it from
//! the pose and reports it separately as a [`Step`], so the caller moves the character entity and
//! the mesh does not slide.

use anyhow::{Result, anyhow};

use crate::anix::{Anix, Pose};
use crate::math::{Quat, Vec3, nlerp, qconj, qmul, qrot, vadd, vlerp, vsub};
use crate::skel::Skeleton;

/// Local (parent-relative) transform of one bone, in centimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bone {
    pub t: Vec3,
    pub r: Quat,
}

/// One transform per skeleton bone.
pub type Skin = Vec<Bone>;

struct Bank {
    anix: Anix,
    bind: Pose,
    node_bone: Vec<Option<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClipId {
    bank: usize,
    clip: usize,
}

pub struct Library {
    banks: Vec<Bank>,
    rest: Skin,
    skeleton_hashes: Vec<u32>,
    parents: Vec<Option<usize>>,
    root: usize,
}

impl Library {
    pub fn new(skeleton: &Skeleton) -> Result<Self> {
        let rest = (0..skeleton.bones.len())
            .map(|i| {
                let (t, r) = skeleton.local_bind(i);
                Bone { t, r }
            })
            .collect();
        let root = skeleton.find("Null").ok_or_else(|| anyhow!("skeleton has no Null (root motion) bone"))?;
        Ok(Self {
            banks: Vec::new(),
            rest,
            skeleton_hashes: skeleton.bones.iter().map(|b| b.name_hash).collect(),
            parents: skeleton.bones.iter().map(|b| b.parent).collect(),
            root,
        })
    }

    /// Add a bank; returns its index for [`Library::clip`].
    pub fn add_bank(&mut self, data: &[u8]) -> Result<usize> {
        let anix = Anix::parse(data)?;
        let bind = anix.bind_pose()?;
        let node_bone = anix
            .node_hashes
            .iter()
            .map(|h| self.skeleton_hashes.iter().position(|s| s == h))
            .collect();
        self.banks.push(Bank { anix, bind, node_bone });
        Ok(self.banks.len() - 1)
    }

    pub fn clip(&self, bank: usize, name: &str) -> Option<ClipId> {
        let b = self.banks.get(bank)?;
        let hash = crate::hash::name_hash(name);
        let clip = b.anix.clips.iter().position(|c| c.name_hash == hash)?;
        Some(ClipId { bank, clip })
    }

    /// The skeleton bone called `name`.
    pub fn find_bone(&self, name: &str) -> Option<usize> {
        let hash = crate::hash::name_hash(name);
        self.skeleton_hashes.iter().position(|&h| h == hash)
    }

    /// The parent of `bone`.
    pub fn parent(&self, bone: usize) -> Option<usize> {
        self.parents[bone]
    }

    /// Model-space position (cm) and orientation of `bone` in `skin`.
    pub fn bone_transform(&self, skin: &Skin, bone: usize) -> (Vec3, Quat) {
        let mut pos = skin[bone].t;
        let mut rot = skin[bone].r;
        let mut at = self.parents[bone];
        while let Some(parent) = at {
            pos = vadd(skin[parent].t, qrot(skin[parent].r, pos));
            rot = qmul(skin[parent].r, rot);
            at = self.parents[parent];
        }
        (pos, rot)
    }

    /// Model-space position (cm) of `bone` in `skin`, by walking up its parents.
    pub fn bone_position(&self, skin: &Skin, bone: usize) -> Vec3 {
        let mut pos = skin[bone].t;
        let mut rot = skin[bone].r;
        let mut at = self.parents[bone];
        while let Some(parent) = at {
            pos = vadd(skin[parent].t, qrot(skin[parent].r, pos));
            rot = qmul(skin[parent].r, rot);
            at = self.parents[parent];
        }
        let _ = rot;
        pos
    }

    /// When (seconds into the clip) `bone` moves fastest inside the window `[from_s, to_s]`: the instant a
    /// swing is at full speed, which is when the blade meets what it is aimed at.
    pub fn fastest_s(&self, id: ClipId, bone: usize, from_s: f32, to_s: f32) -> Option<f32> {
        let start = self.start_ms(id);
        let step = 33.0;
        let mut best: Option<(f32, f32)> = None;
        let mut t = from_s * 1000.0;
        let mut prev = self.bone_position(&self.sample(id, start + t).ok()?, bone);
        while t + step <= to_s * 1000.0 {
            let here = self.bone_position(&self.sample(id, start + t + step).ok()?, bone);
            let d = vsub(here, prev);
            let speed = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if best.is_none_or(|(_, b)| speed > b) {
                best = Some(((t + step * 0.5) / 1000.0, speed));
            }
            prev = here;
            t += step;
        }
        best.map(|b| b.0)
    }

    /// The first clip called `name` in any bank.
    pub fn find(&self, name: &str) -> Option<ClipId> {
        (0..self.banks.len()).find_map(|bank| self.clip(bank, name))
    }

    /// Every clip of a bank (including `bind`), with its name.
    pub fn clips(&self, bank: usize) -> impl Iterator<Item = (ClipId, &str)> {
        self.banks
            .get(bank)
            .into_iter()
            .flat_map(move |b| b.anix.clips.iter().enumerate().map(move |(clip, c)| (ClipId { bank, clip }, c.name.as_str())))
    }

    pub fn bank_count(&self) -> usize {
        self.banks.len()
    }

    /// Total root motion of a clip from start to end: travel in its start frame (cm) and yaw (game radians,
    /// wrapped to +-pi).
    pub fn root_total(&self, id: ClipId) -> Result<(Vec3, f32)> {
        let start = self.sample(id, self.start_ms(id))?[self.root];
        let end = self.sample(id, self.start_ms(id) + self.duration_ms(id))?[self.root];
        Ok(root_step(start, end))
    }

    /// Root travel from the clip start to `t` seconds into it, in the start frame (cm): the clip's own path.
    pub fn root_travel_at(&self, id: ClipId, t: f32) -> Result<Vec3> {
        let start = self.sample(id, self.start_ms(id))?[self.root];
        let ms = (self.start_ms(id) + t * 1000.0).min(self.start_ms(id) + self.duration_ms(id));
        let at = self.sample(id, ms)?[self.root];
        Ok(root_step(start, at).0)
    }

    /// Timed cues of a clip (hit frames, windows, audio hooks).
    pub fn events(&self, id: ClipId) -> &[crate::anix::Event] {
        &self.banks[id.bank].anix.clips[id.clip].events
    }

    /// Time in seconds, from the clip start, of the first cue containing `name`.
    pub fn event_s(&self, id: ClipId, name: &str) -> Option<f32> {
        let clip = &self.banks[id.bank].anix.clips[id.clip];
        let scale = self.cue_scale(id);
        // The engine's damage cue handler also takes the short `AD` form (knock-down moves use it).
        clip.event_ms(name)
            .or_else(|| if name == "APPLYDAMAGE" { clip.event_ms("AD") } else { None })
            .map(|t| (t as f32 - clip.start_ms() as f32) / 1000.0 * scale)
    }

    /// Some clips (orc attacks, sync pairs) carry cues on a longer timeline than their key data: the last cue
    /// (the animation-end marker) lies well past the clip end. Their cue times are then stretched by
    /// `duration / last cue`; the blow of such a clip falls where the arm actually swings, not after the clip.
    pub fn cue_scale(&self, id: ClipId) -> f32 {
        let clip = &self.banks[id.bank].anix.clips[id.clip];
        let duration = clip.duration_ms() as f32;
        let last = clip.events.iter().map(|e| e.time_ms).max().unwrap_or(0) as f32 - clip.start_ms() as f32;
        if duration > 0.0 && last > duration * 1.15 { duration / last } else { 1.0 }
    }

    pub fn name(&self, id: ClipId) -> &str {
        &self.banks[id.bank].anix.clips[id.clip].name
    }

    pub fn start_ms(&self, id: ClipId) -> f32 {
        self.banks[id.bank].anix.clips[id.clip].start_ms() as f32
    }

    pub fn duration_ms(&self, id: ClipId) -> f32 {
        self.banks[id.bank].anix.clips[id.clip].duration_ms() as f32
    }

    pub fn rest(&self) -> &Skin {
        &self.rest
    }

    pub fn root_bone(&self) -> usize {
        self.root
    }

    /// The full skin of `id` at absolute clip time `time_ms`.
    pub fn sample(&self, id: ClipId, time_ms: f32) -> Result<Skin> {
        let bank = &self.banks[id.bank];
        let clip = &bank.anix.clips[id.clip];
        let pose = bank.anix.sample(clip, time_ms.max(0.0) as u32, &bank.bind)?;
        let mut skin = self.rest.clone();
        for (node, bone) in bank.node_bone.iter().enumerate() {
            if let Some(bone) = *bone {
                skin[bone] = Bone { t: pose.translation[node], r: pose.rotation[node] };
            }
        }
        Ok(skin)
    }
}

#[derive(Debug, Clone)]
struct Layer {
    clip: ClipId,
    time_ms: f32,
    looped: bool,
    ended: bool,
    weight: f32,
    target: f32,
    /// Root bone at the last update, to take the next step from.
    root: Bone,
}

/// Result of advancing the animator by one frame.
#[derive(Debug, Clone)]
pub struct Step {
    /// Blended pose with the root bone reset to its bind transform.
    pub pose: Skin,
    /// Root travel this frame in the character's own frame (cm; x right, y up, z forward).
    pub travel: Vec3,
    /// Root yaw this frame in radians (game space, positive turns from +Z toward +X).
    pub yaw: f32,
}

#[derive(Default)]
pub struct Animator {
    layers: Vec<Layer>,
    fade_s: f32,
}

impl Animator {
    pub fn new() -> Self {
        Self::default()
    }

    /// The clip fading in (the most recently started one).
    pub fn current(&self) -> Option<ClipId> {
        self.layers.last().map(|l| l.clip)
    }

    /// Position of the current clip as 0..1 of its length.
    pub fn phase(&self, lib: &Library) -> f32 {
        self.layers.last().map_or(0.0, |l| {
            let d = lib.duration_ms(l.clip).max(1.0);
            ((l.time_ms - lib.start_ms(l.clip)) / d).clamp(0.0, 1.0)
        })
    }

    /// A one-shot clip has reached its end.
    pub fn ended(&self) -> bool {
        self.layers.last().is_some_and(|l| l.ended)
    }

    /// Seconds into the current clip.
    pub fn elapsed_s(&self, lib: &Library) -> f32 {
        self.layers.last().map_or(0.0, |l| (l.time_ms - lib.start_ms(l.clip)) / 1000.0)
    }

    /// Start `clip`, fading the others out over `fade_s`. Does nothing if it is already current
    /// (and still playing). `phase` starts it part-way through, to keep walk and run cycles in step.
    pub fn play(&mut self, lib: &Library, clip: ClipId, looped: bool, fade_s: f32, phase: f32) -> Result<()> {
        if self.layers.last().is_some_and(|l| l.clip == clip && !l.ended) {
            return Ok(());
        }
        self.restart(lib, clip, looped, fade_s, phase)
    }

    /// Like [`Animator::play`], but starts the clip over even if it is the one already playing (an attack chained into itself).
    pub fn restart(&mut self, lib: &Library, clip: ClipId, looped: bool, fade_s: f32, phase: f32) -> Result<()> {
        let time_ms = lib.start_ms(clip) + phase.clamp(0.0, 1.0) * lib.duration_ms(clip);
        let root = lib.sample(clip, time_ms)?[lib.root_bone()];
        let instant = self.layers.is_empty() || fade_s <= 0.0;
        for layer in &mut self.layers {
            layer.target = 0.0;
        }
        if instant {
            self.layers.clear();
        }
        self.fade_s = fade_s.max(0.0);
        self.layers.push(Layer {
            clip,
            time_ms,
            looped,
            ended: false,
            weight: if instant { 1.0 } else { 0.0 },
            target: 1.0,
            root,
        });
        Ok(())
    }

    pub fn update(&mut self, lib: &Library, dt_s: f32) -> Result<Step> {
        let root_bone = lib.root_bone();
        let mut pose: Option<(Skin, f32)> = None;
        let mut travel = [0.0; 3];
        let mut yaw = 0.0;

        let total: f32 = self.layers.iter().map(|l| l.weight.max(0.001)).sum();
        for layer in &mut self.layers {
            let (start, duration) = (lib.start_ms(layer.clip), lib.duration_ms(layer.clip).max(1.0));
            let end = start + duration;
            let previous = layer.time_ms;
            let mut now = previous + dt_s * 1000.0;
            let mut wrapped = false;
            if now >= end {
                if layer.looped {
                    now = start + (now - start) % duration;
                    wrapped = true;
                } else {
                    now = end;
                    layer.ended = true;
                }
            }
            layer.time_ms = now;
            let skin = lib.sample(layer.clip, now)?;

            // Root step since the last update, taken across the loop point when the clip wrapped.
            let root_now = skin[root_bone];
            let step = if wrapped {
                let at_end = lib.sample(layer.clip, end)?[root_bone];
                let at_start = lib.sample(layer.clip, start)?[root_bone];
                let (a, ay) = root_step(layer.root, at_end);
                let (b, by) = root_step(at_start, root_now);
                ([a[0] + b[0], a[1] + b[1], a[2] + b[2]], ay + by)
            } else {
                root_step(layer.root, root_now)
            };
            layer.root = root_now;

            let w = layer.weight.max(0.001) / total;
            travel = [travel[0] + step.0[0] * w, travel[1] + step.0[1] * w, travel[2] + step.0[2] * w];
            yaw += step.1 * w;

            pose = Some(match pose {
                None => (skin, w),
                Some((acc, aw)) => {
                    let mix = w / (aw + w);
                    (blend(&acc, &skin, mix), aw + w)
                }
            });

            // Fade toward the target weight.
            let rate = if self.fade_s > 0.0 { dt_s / self.fade_s } else { 1.0 };
            layer.weight += (layer.target - layer.weight).clamp(-rate, rate);
        }
        // The newest layer fully in means the older ones can go.
        if self.layers.last().is_some_and(|l| l.weight >= 1.0) {
            let keep = self.layers.len() - 1;
            self.layers.drain(..keep);
        }
        self.layers.retain(|l| l.weight > 0.0 || l.target > 0.0);

        let mut pose = pose.map(|p| p.0).unwrap_or_else(|| lib.rest().clone());
        pose[root_bone] = lib.rest()[root_bone];
        Ok(Step { pose, travel, yaw })
    }
}

fn blend(a: &Skin, b: &Skin, t: f32) -> Skin {
    a.iter().zip(b).map(|(x, y)| Bone { t: vlerp(x.t, y.t, t), r: nlerp(x.r, y.r, t) }).collect()
}

/// Motion from root transform `a` to `b`, expressed in `a`'s frame.
fn root_step(a: Bone, b: Bone) -> (Vec3, f32) {
    let inv = qconj(a.r);
    let d = qrot(inv, [b.t[0] - a.t[0], b.t[1] - a.t[1], b.t[2] - a.t[2]]);
    let q = qmul(inv, b.r);
    let mut yaw = 2.0 * q[1].atan2(q[3]);
    if yaw > std::f32::consts::PI {
        yaw -= std::f32::consts::TAU;
    } else if yaw < -std::f32::consts::PI {
        yaw += std::f32::consts::TAU;
    }
    (d, yaw)
}
