//! `SKEL` v8 skeletons.
//!
//! ```text
//! 0x00  "SKEL", u32 version (8)
//! 0x08  u32 x14: [2]=64 [3]=name blob size [4]=? [5]=bone count [6]=bone bytes (37 * count)
//!                [7]=? [8]=piece count [9]=? [10]=byte-table size [11..15]=?
//!       then 7 u32 (a bounding box as floats)
//! 0x5c  piece tables: per piece u32 n, then n * 11 bytes
//!       u32 0xDCDCFF00
//!       name blob (NUL-terminated strings, offsets referenced below)
//!       the bone tree, depth first, root first, 37 bytes per bone:
//!         u32 name offset, u8 flags, f32 translation[3], f32 rotation[4] (x y z w),
//!         u32 child count
//!       ... further sections (sockets, hierarchies, ...) that are not read here
//! ```
//!
//! The tree stores each bone's transform in **model space** (bind pose), in centimetres, Y up.
//! Animations are expressed as parent-relative transforms; [`Skeleton::local_bind`] derives
//! those from the model-space values.

use anyhow::{Result, ensure};

use crate::hash::name_hash;
use crate::math::{Quat, Vec3, qconj, qmul, qrot, vsub};
use crate::{cstr_at, u32_at};

const MARKER_PIECES: u32 = 0xDCDC_FF00;
const BONE_BYTES: usize = 37;

#[derive(Debug, Clone)]
pub struct Bone {
    pub name: String,
    pub name_hash: u32,
    pub parent: Option<usize>,
    pub flags: u8,
    /// Model-space bind position (cm).
    pub translation: Vec3,
    /// Model-space bind orientation.
    pub rotation: Quat,
}

#[derive(Debug, Clone)]
pub struct Skeleton {
    /// In file (depth-first) order.
    pub bones: Vec<Bone>,
    /// `engine_order[i]` is the bone the engine calls index `i` (see [`engine_order`]).
    /// Mesh bone palettes use these indices.
    pub engine_order: Vec<usize>,
}

/// The engine reads the tree depth first but stores each node's children in one contiguous
/// block, allocated when the node is read. Given the child count of every node in
/// depth-first order, returns the depth-first index of each engine slot.
pub fn engine_order(child_counts: &[u32]) -> Result<Vec<usize>> {
    let mut order = vec![0usize; child_counts.len()];
    let mut next = 1usize;
    // (first slot, slot count, slots handed out)
    let mut blocks: Vec<(usize, usize, usize)> = Vec::new();
    for (node, &children) in child_counts.iter().enumerate() {
        let slot = if node == 0 {
            0
        } else {
            loop {
                let Some(block) = blocks.last_mut() else { anyhow::bail!("bone tree has more than one root") };
                if block.2 < block.1 {
                    block.2 += 1;
                    break block.0 + block.2 - 1;
                }
                blocks.pop();
            }
        };
        ensure!(slot < order.len(), "bone tree child counts exceed the bone count");
        order[slot] = node;
        if children > 0 {
            blocks.push((next, children as usize, 0));
            next += children as usize;
        }
    }
    Ok(order)
}

impl Skeleton {
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(data.len() > 0x5c && &data[..4] == b"SKEL", "not a SKEL file");
        let version = u32_at(data, 4)?;
        ensure!(version == 8, "unsupported SKEL version {version}");
        let blob_size = u32_at(data, 0x0c)? as usize;
        let bone_count = u32_at(data, 0x14)? as usize;
        let piece_count = u32_at(data, 0x20)? as usize;

        let mut pos = 0x5c;
        for _ in 0..piece_count {
            pos += 4 + 11 * u32_at(data, pos)? as usize;
        }
        ensure!(u32_at(data, pos)? == MARKER_PIECES, "missing section marker after piece tables");
        pos += 4;
        let blob = data.get(pos..pos + blob_size).ok_or_else(|| anyhow::anyhow!("name blob out of range"))?;
        pos += blob_size;
        ensure!(
            data.len() >= pos + bone_count * BONE_BYTES,
            "bone tree runs past the end of the file"
        );

        let mut bones: Vec<Bone> = Vec::with_capacity(bone_count);
        let mut child_counts = Vec::with_capacity(bone_count);
        // Depth-first with an explicit stack of (parent index, children still to read).
        let mut stack: Vec<(usize, u32)> = Vec::new();
        while bones.len() < bone_count {
            let at = pos + bones.len() * BONE_BYTES;
            let name = cstr_at(blob, u32_at(data, at)? as usize)?;
            let f = |i: usize| f32::from_le_bytes(data[at + 5 + 4 * i..at + 9 + 4 * i].try_into().unwrap());
            let index = bones.len();
            let parent = loop {
                match stack.last_mut() {
                    Some((p, left)) if *left > 0 => {
                        *left -= 1;
                        break Some(*p);
                    }
                    Some(_) => {
                        stack.pop();
                    }
                    None => break None,
                }
            };
            ensure!(parent.is_some() == (index > 0), "bone tree has more than one root");
            let children = u32_at(data, at + 33)?;
            child_counts.push(children);
            bones.push(Bone {
                name_hash: name_hash(&name),
                name,
                parent,
                flags: data[at + 4],
                translation: [f(0), f(1), f(2)],
                rotation: [f(3), f(4), f(5), f(6)],
            });
            if children > 0 {
                stack.push((index, children));
            }
        }
        let engine_order = engine_order(&child_counts)?;
        Ok(Self { bones, engine_order })
    }

    /// A plain humanoid stick figure of our own (T-pose, 1.8 m, identity orientations): the body chain the animations drive, by the
    /// names the animation banks use. No game skeleton data.
    pub fn humanoid() -> Self {
        // (name, parent, model-space position in cm)
        let spec: &[(&str, Option<usize>, Vec3)] = &[
            ("Null", None, [0.0, 0.0, 0.0]),
            ("Pelvis", Some(0), [0.0, 100.0, 0.0]),
            ("Torso_Base", Some(1), [0.0, 108.0, 0.0]),
            ("Torso", Some(2), [0.0, 120.0, 0.0]),
            ("Upper_Torso", Some(3), [0.0, 135.0, 0.0]),
            ("Spine4_joint", Some(4), [0.0, 145.0, 0.0]),
            ("Neck", Some(5), [0.0, 152.0, 0.0]),
            ("Head", Some(6), [0.0, 162.0, 0.0]),
            ("L_Shoulder", Some(5), [8.0, 148.0, 0.0]),
            ("L_Armu", Some(8), [18.0, 148.0, 0.0]),
            ("L_Arml", Some(9), [45.0, 148.0, 0.0]),
            ("L_Hand", Some(10), [72.0, 148.0, 0.0]),
            ("L_Weapon", Some(11), [80.0, 148.0, 0.0]),
            ("R_Shoulder", Some(5), [-8.0, 148.0, 0.0]),
            ("R_Armu", Some(13), [-18.0, 148.0, 0.0]),
            ("R_Arml", Some(14), [-45.0, 148.0, 0.0]),
            ("R_Hand", Some(15), [-72.0, 148.0, 0.0]),
            ("R_Weapon", Some(16), [-80.0, 148.0, 0.0]),
            ("L_Legu", Some(1), [10.0, 95.0, 0.0]),
            ("L_Legl", Some(18), [10.0, 52.0, 0.0]),
            ("L_Foot", Some(19), [10.0, 10.0, 0.0]),
            ("L_Toe", Some(20), [10.0, 4.0, 14.0]),
            ("R_Legu", Some(1), [-10.0, 95.0, 0.0]),
            ("R_Legl", Some(22), [-10.0, 52.0, 0.0]),
            ("R_Foot", Some(23), [-10.0, 10.0, 0.0]),
            ("R_Toe", Some(24), [-10.0, 4.0, 14.0]),
        ];
        let bones: Vec<Bone> = spec
            .iter()
            .map(|&(name, parent, translation)| Bone { name: name.to_string(), name_hash: name_hash(name), parent, flags: 0, translation, rotation: [0.0, 0.0, 0.0, 1.0] })
            .collect();
        let engine_order = (0..bones.len()).collect();
        Self { bones, engine_order }
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        let hash = name_hash(name);
        self.bones.iter().position(|b| b.name_hash == hash)
    }

    pub fn find_hash(&self, hash: u32) -> Option<usize> {
        self.bones.iter().position(|b| b.name_hash == hash)
    }

    /// Parent-relative bind transform of `bone`.
    pub fn local_bind(&self, bone: usize) -> (Vec3, Quat) {
        let b = &self.bones[bone];
        match b.parent {
            None => (b.translation, b.rotation),
            Some(p) => {
                let parent = &self.bones[p];
                let inv = qconj(parent.rotation);
                (qrot(inv, vsub(b.translation, parent.translation)), qmul(inv, b.rotation))
            }
        }
    }
}
