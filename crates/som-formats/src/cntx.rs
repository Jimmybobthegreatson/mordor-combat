//! Context actions: the lines the game's traversal runs on (ledges to hang from, tops to pull up onto, beams to run
//! along, places to stop). The behaviour graph (`behaviors/player/player_elf.bvr`) searches them with its
//! `FindContextAction` / `Predicate.FoundContextAction` nodes; nothing about climbing is derived from the render mesh.
//!
//! They are stored as a `CADB` block, one per streaming stamp (`worlds/**.ss_*.cntx`, in the stamp's own frame) and one
//! for the persistent world inside its `.objres`:
//!
//! ```text
//! 0x00  "CADB", u32 version (1), u32 line count, u32 type count, u32 group count, u32 group bytes / 4 (?)
//! 0x18  u32 ? (absent: types start at 0x1c)
//! 0x1c  u32 type name hashes[type count]      bit i of a line's flags = type i (the order differs per file)
//!       lines[line count], 52 bytes: f32 a[3], f32 b[3], f32 normal[3] (cm; the normal is horizontal and points
//!                                    into the wall: the way the character faces when he hangs there), u64 flags, u64 0
//!       dynamic groups (bounds and line ranges of movable sets), not read
//! ```
//!
//! Prefabs carry no lines of their own: the world stores them per placement. [`placements`] finds where a prefab is
//! placed in a world (`.wldclnt` instance records) and [`prefab_lines`] brings the lines on those placements back
//! into the prefab's frame, keeping only the lines every placement agrees on.

use anyhow::{Result, bail, ensure};

use crate::hash::name_hash;
use crate::math::{Quat, Vec3, qconj, qrot, vsub};
use crate::u32_at;

/// The context action types by name; a file names its types by hash and the order varies.
pub const TYPE_NAMES: &[&str] = &[
    "Edge_Hang",
    "Edge_Hang_Dangle",
    "Edge_Hang_Rope",
    "Edge_Hang_Rail_Detour",
    "Edge_Pullup",
    "Edge_PullupToSidle",
    "Edge_NoHang",
    "Rail_Running",
    "Rail_Sliding",
    "Block_Movement",
    "Block_Movement_RailRun",
    "PreventHopping",
    "Go_Left_If_No_Up",
    "Go_Right_If_No_Up",
    "VaultOver",
    "Hurdle",
    "BoxJump",
    "Polejump",
    "PoleJump",
    "ZipLine",
    "Narrow_Space",
    "Narrow_Space_Hole",
    "Stealth",
    "Stealth_High",
    "Stealth_Low",
];

#[derive(Debug, Clone, Copy)]
pub struct Line {
    pub a: Vec3,
    pub b: Vec3,
    pub normal: Vec3,
    pub flags: u64,
}

#[derive(Debug, Clone)]
pub struct ContextDb {
    /// Name hash of each type bit.
    pub types: Vec<u32>,
    pub lines: Vec<Line>,
}

impl ContextDb {
    /// Parse a `CADB` block starting at `data[0]`.
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(data.len() > 0x1c && &data[..4] == b"CADB", "not a CADB block");
        let version = u32_at(data, 4)?;
        ensure!(version == 1, "unsupported CADB version {version}");
        let count = u32_at(data, 8)? as usize;
        let type_count = u32_at(data, 12)? as usize;
        ensure!(type_count <= 64, "{type_count} context action types");
        let types = (0..type_count).map(|i| u32_at(data, 0x1c + 4 * i)).collect::<Result<Vec<_>>>()?;
        let base = 0x1c + 4 * type_count;
        ensure!(base + count * 52 <= data.len(), "CADB lines run past the data");
        let f = |o: usize| -> Result<f32> { Ok(f32::from_bits(u32_at(data, o)?)) };
        let mut lines = Vec::with_capacity(count);
        for i in 0..count {
            let o = base + i * 52;
            let v = |k: usize| -> Result<Vec3> { Ok([f(o + 4 * k)?, f(o + 4 * k + 4)?, f(o + 4 * k + 8)?]) };
            let flags = u32_at(data, o + 36)? as u64 | (u32_at(data, o + 40)? as u64) << 32;
            lines.push(Line { a: v(0)?, b: v(3)?, normal: v(6)?, flags });
        }
        Ok(Self { types, lines })
    }

    /// The first `CADB` block inside a larger file (a world's `.objres`), or a `.cntx` file itself.
    pub fn find(data: &[u8]) -> Result<Self> {
        match data.windows(4).position(|w| w == b"CADB") {
            Some(at) => Self::parse(&data[at..]),
            None => bail!("no CADB block"),
        }
    }

    /// The flag bit of a type, if this database has it.
    pub fn bit(&self, name: &str) -> Option<u64> {
        let hash = name_hash(name);
        self.types.iter().position(|&h| h == hash).map(|i| 1u64 << i)
    }

    /// Names of the types set on `line` (unknown ones by hash).
    pub fn names(&self, line: &Line) -> Vec<String> {
        (0..self.types.len())
            .filter(|i| line.flags >> i & 1 == 1)
            .map(|i| {
                TYPE_NAMES
                    .iter()
                    .find(|n| name_hash(n) == self.types[i])
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| format!("{:08x}", self.types[i]))
            })
            .collect()
    }

    /// The lines of this database re-expressed with a fixed type order ([`TYPE_NAMES`]): bit i = `TYPE_NAMES[i]`.
    pub fn canonical_flags(&self, line: &Line) -> u64 {
        let mut out = 0;
        for (i, &h) in self.types.iter().enumerate() {
            if line.flags >> i & 1 == 1 {
                if let Some(k) = TYPE_NAMES.iter().position(|n| name_hash(n) == h) {
                    out |= 1 << k;
                }
            }
        }
        out
    }
}

/// Canonical bits ([`TYPE_NAMES`] order) of the types traversal uses.
pub const EDGE_HANG: u64 = 1 << 0;
pub const EDGE_HANG_DANGLE: u64 = 1 << 1;
pub const EDGE_PULLUP: u64 = 1 << 4;
pub const EDGE_NO_HANG: u64 = 1 << 6;
pub const RAIL_RUNNING: u64 = 1 << 7;
pub const BLOCK_MOVEMENT: u64 = 1 << 9;

/// Bit of `name` in [`ContextDb::canonical_flags`].
pub fn canonical_bit(name: &str) -> u64 {
    TYPE_NAMES.iter().position(|n| *n == name).map_or(0, |i| 1 << i)
}

/// One placement of a prefab in a world.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub position: Vec3,
    /// x, y, z, w.
    pub rotation: Quat,
    pub scale: f32,
}

/// Where `prefab` (an asset path as the world spells it, any case or slash) is placed in a world client file
/// (`worlds/**.wldclnt`, `WDSC` v5). Instance records are 19 words: bounds min[3], max[3], position[3],
/// rotation[4], scale, 1, 0, type hash, 0, offset of the prefab path in the name table that starts at 0x2c.
pub fn placements(wldclnt: &[u8], prefab: &str) -> Result<Vec<Placement>> {
    ensure!(wldclnt.len() > 0x2c && &wldclnt[..4] == b"WDSC", "not a world client file");
    let wanted: Vec<u8> = prefab.bytes().map(|b| if b == b'/' { b'\\' } else { b.to_ascii_lowercase() }).collect();
    let Some(at) = wldclnt
        .windows(wanted.len())
        .position(|w| w.iter().map(|b| b.to_ascii_lowercase()).eq(wanted.iter().copied()))
    else {
        return Ok(Vec::new());
    };
    let offset = (at - 0x2c) as u32;
    let f = |o: usize| f32::from_bits(u32::from_le_bytes(wldclnt[o..o + 4].try_into().unwrap()));
    let mut out = Vec::new();
    let start = (at + wanted.len() + 3) & !3;
    let mut o = start.max(19 * 4);
    while o + 4 <= wldclnt.len() {
        if u32::from_le_bytes(wldclnt[o..o + 4].try_into().unwrap()) == offset {
            let r = o - 18 * 4;
            let rotation = [f(r + 36), f(r + 40), f(r + 44), f(r + 48)];
            let norm = rotation.iter().map(|x| x * x).sum::<f32>();
            let scale = f(r + 52);
            if (norm - 1.0).abs() < 0.01 && scale > 0.05 && scale < 20.0 {
                out.push(Placement { position: [f(r + 24), f(r + 28), f(r + 32)], rotation, scale });
            }
        }
        o += 4;
    }
    Ok(out)
}

fn to_local(p: &Placement, v: Vec3) -> Vec3 {
    let l = qrot(qconj(p.rotation), vsub(v, p.position));
    [l[0] / p.scale, l[1] / p.scale, l[2] / p.scale]
}

fn dist(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The lines of a prefab in its own frame (cm): the world's lines on each placement brought back through the
/// placement transform, within `bounds` (prefab frame, cm) grown by `margin`; a line is kept when at least half of the
/// other placements have the same line (both ends within 15 cm), so neighbours' lines drop out. Flags are canonical.
pub fn prefab_lines(db: &ContextDb, at: &[Placement], bounds: (Vec3, Vec3), margin: f32) -> Vec<Line> {
    let inside = |v: Vec3| (0..3).all(|k| v[k] >= bounds.0[k] - margin && v[k] <= bounds.1[k] + margin);
    let per: Vec<Vec<Line>> = at
        .iter()
        .map(|p| {
            db.lines
                .iter()
                .filter_map(|l| {
                    let (a, b) = (to_local(p, l.a), to_local(p, l.b));
                    (inside(a) && inside(b)).then(|| Line {
                        a,
                        b,
                        normal: qrot(qconj(p.rotation), l.normal),
                        flags: db.canonical_flags(l),
                    })
                })
                .collect()
        })
        .collect();
    let Some((first, others)) = per.split_first() else { return Vec::new() };
    let same = |x: &Line, y: &Line| {
        (dist(x.a, y.a) < 15.0 && dist(x.b, y.b) < 15.0) || (dist(x.a, y.b) < 15.0 && dist(x.b, y.a) < 15.0)
    };
    first
        .iter()
        .filter(|l| others.is_empty() || others.iter().filter(|o| o.iter().any(|m| same(l, m))).count() * 2 >= others.len())
        .copied()
        .collect()
}
