//! `ANIX` v42 animation banks.
//!
//! A bank holds a `bind` pose plus any number of named clips, all over one list of nodes
//! (bones, identified by [`name_hash`]). Times are in milliseconds.
//!
//! ```text
//! 0x00  "ANIX", u32 version (42), u32 file size, u32 0x20
//! 0x10  u32 block A size, u32 block B offset, u32 block B size
//! 0x20  block A: key data, one chunk per clip segment
//!       block B: descriptors
//!
//! block B
//!   0x00 u32 group table offset   0x04 u32 group count
//!   0x0c u32 node count           0x10 u32 channel count
//!   0x18 u32 node name hashes[], then u32 channel name hashes[] ("Pos", "Rotation", ...)
//!   group table: { u32 name hash, u32 group offset } per group
//!
//! group (clip)
//!   +0x00 name (NUL padded)     +0x34 u16 segment count
//!   +0x38 u16 codec count       +0x3a u16 element count
//!   at floor4(+0x77): codec tags (4CC) [codec count]
//!   then elements [element count + 1]: { u8 channel, u8 node, u8 codec }
//!   at floor4(end): segments: { u32 A offset, u32 A size, u32 start ms, u32 end ms }
//!
//! segment data (in block A)
//!   u32 named block count N, u16 codec data offset[codec count], u16 named offset[N],
//!   (align 4) u32 named hash[N], then the codec blocks. Offsets are from the segment start.
//! ```
//!
//! Each codec block decodes the elements assigned to it, in element order. Codecs seen on
//! the player: `SV01`/`AV01` (static/animated 10:10:10 vectors), `SQ03`/`AQ03` (static/animated
//! smallest-three quaternions, 13 bits per component), `UATR` (float vector palette).

use std::collections::HashMap;

use anyhow::{Result, anyhow, bail, ensure};

use crate::hash::name_hash;
use crate::math::{Quat, Vec3, nlerp, vlerp};

#[derive(Debug, Clone)]
struct Segment {
    offset: usize,
    size: usize,
    start_ms: u32,
    end_ms: u32,
}

#[derive(Debug, Clone, Copy)]
struct Element {
    channel: u8,
    node: u8,
    codec: u8,
}

/// A timed event baked into a clip: game-logic cues such as `APPLYDAMAGE`, hit windows (`BHITWIN` /
/// `EHITWIN`), the buffered-transition point (`BUFFTRANS`) and audio hooks.
#[derive(Debug, Clone)]
pub struct Event {
    pub time_ms: u32,
    /// The raw cue text, e.g. `RFACE Angry; BUFFTRANS; EHITWIN;`.
    pub text: String,
}

impl Event {
    /// Whether the cue text contains the word `name` (cues are `;`/space separated words).
    pub fn has(&self, name: &str) -> bool {
        self.text.split(|c: char| c == ';' || c.is_whitespace()).any(|w| w.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Clone)]
pub struct Clip {
    pub name: String,
    pub name_hash: u32,
    /// Codec tags used by this clip, e.g. `["AQ03", "STRI", "UATR", "SV01", "SQ03"]`.
    pub codecs: Vec<String>,
    elements: Vec<Element>,
    segments: Vec<Segment>,
    /// Timed cues, in time order.
    pub events: Vec<Event>,
}

impl Clip {
    pub fn start_ms(&self) -> u32 {
        self.segments.first().map_or(0, |s| s.start_ms)
    }

    pub fn end_ms(&self) -> u32 {
        self.segments.last().map_or(0, |s| s.end_ms)
    }

    /// `(start_ms, end_ms)` of every segment, for diagnosis.
    pub fn segment_ranges(&self) -> Vec<(u32, u32)> {
        self.segments.iter().map(|s| (s.start_ms, s.end_ms)).collect()
    }

    pub fn duration_ms(&self) -> u32 {
        self.end_ms() - self.start_ms()
    }

    /// Time of the first event whose cue text contains the word `name`.
    pub fn event_ms(&self, name: &str) -> Option<u32> {
        self.events.iter().find(|e| e.has(name)).map(|e| e.time_ms)
    }
}

/// Parent-relative transforms, one per bank node.
#[derive(Debug, Clone)]
pub struct Pose {
    pub translation: Vec<Vec3>,
    pub rotation: Vec<Quat>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Channel {
    Position,
    Rotation,
    Other,
}

pub struct Anix {
    a: Vec<u8>,
    /// Name hash of each node; match against `Bone::name_hash`.
    pub node_hashes: Vec<u32>,
    channels: Vec<Channel>,
    channel_hashes: Vec<u32>,
    /// Every group in file order, including `bind`.
    pub clips: Vec<Clip>,
}

fn u16_at(buf: &[u8], off: usize) -> Result<u16> {
    let b = buf.get(off..off + 2).ok_or_else(|| anyhow!("read past end at {off:#x}"))?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(buf: &[u8], off: usize) -> Result<u32> {
    crate::u32_at(buf, off)
}

fn u64_at(buf: &[u8], off: usize) -> Result<u64> {
    crate::u64_at(buf, off)
}

fn f32_at(buf: &[u8], off: usize) -> Result<f32> {
    Ok(f32::from_bits(u32_at(buf, off)?))
}

impl Anix {
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(data.len() >= 0x20 && &data[..4] == b"ANIX", "not an ANIX file");
        let version = u32_at(data, 4)?;
        ensure!(version == 42, "unsupported ANIX version {version}");
        let a_size = u32_at(data, 0x10)? as usize;
        let b_off = u32_at(data, 0x14)? as usize;
        let b_size = u32_at(data, 0x18)? as usize;
        let a = data.get(0x20..0x20 + a_size).ok_or_else(|| anyhow!("block A out of range"))?.to_vec();
        let b = data.get(b_off..b_off + b_size).ok_or_else(|| anyhow!("block B out of range"))?;

        let group_table = u32_at(b, 0)? as usize;
        let group_count = u32_at(b, 4)? as usize;
        let node_count = u32_at(b, 0xc)? as usize;
        let channel_count = u32_at(b, 0x10)? as usize;
        let node_hashes =
            (0..node_count).map(|i| u32_at(b, 0x18 + 4 * i)).collect::<Result<Vec<_>>>()?;
        let (pos, rot) = (name_hash("Pos"), name_hash("Rotation"));
        let channel_hashes = (0..channel_count)
            .map(|i| u32_at(b, 0x18 + 4 * (node_count + i)))
            .collect::<Result<Vec<_>>>()?;
        let channels = (0..channel_count)
            .map(|i| {
                Ok(match u32_at(b, 0x18 + 4 * (node_count + i))? {
                    h if h == pos => Channel::Position,
                    h if h == rot => Channel::Rotation,
                    _ => Channel::Other,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mut clips = Vec::with_capacity(group_count);
        for g in 0..group_count {
            let hash = u32_at(b, group_table + 8 * g)?;
            let at = u32_at(b, group_table + 8 * g + 4)? as usize;
            let name_bytes = b.get(at..at + 0x32).ok_or_else(|| anyhow!("group header out of range"))?;
            let name: String =
                name_bytes.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
            let segment_count = u16_at(b, at + 0x34)? as usize;
            let codec_count = u16_at(b, at + 0x38)? as usize;
            let element_count = u16_at(b, at + 0x3a)? as usize;

            let tags = (at + 0x77) & !3;
            let codecs = (0..codec_count)
                .map(|c| {
                    b.get(tags + 4 * c..tags + 4 * c + 4)
                        .map(|t| t.iter().map(|&x| x as char).collect::<String>())
                        .ok_or_else(|| anyhow!("codec tags out of range"))
                })
                .collect::<Result<Vec<_>>>()?;
            let recs = tags + 4 * codec_count;
            let elements = (0..element_count)
                .map(|e| {
                    let r = b.get(recs + 3 * e..recs + 3 * e + 3).ok_or_else(|| anyhow!("elements out of range"))?;
                    ensure!((r[1] as usize) < node_count, "{name}: element node {} out of range", r[1]);
                    ensure!((r[2] as usize) < codec_count, "{name}: element codec {} out of range", r[2]);
                    Ok(Element { channel: r[0], node: r[1], codec: r[2] })
                })
                .collect::<Result<Vec<_>>>()?;
            // The table holds one extra (unused) element before the segment list.
            let table = (recs + 3 * (element_count + 1)) & !3;
            let segments = (0..segment_count)
                .map(|s| {
                    let e = table + 16 * s;
                    let seg = Segment {
                        offset: u32_at(b, e)? as usize,
                        size: u32_at(b, e + 4)? as usize,
                        start_ms: u32_at(b, e + 8)?,
                        end_ms: u32_at(b, e + 12)?,
                    };
                    ensure!(seg.offset + seg.size <= a.len(), "{name}: segment outside block A");
                    Ok(seg)
                })
                .collect::<Result<Vec<_>>>()?;
            let events = read_events(&a, b, at, table + 16 * segment_count, &codecs, &elements, &segments).unwrap_or_default();
            clips.push(Clip { name, name_hash: hash, codecs, elements, segments, events });
        }
        Ok(Self { a, node_hashes, channels, channel_hashes, clips })
    }

    pub fn clip(&self, name: &str) -> Option<&Clip> {
        let hash = name_hash(name);
        self.clips.iter().find(|c| c.name_hash == hash)
    }

    /// The bank's reference pose (the `bind` group), which every clip is layered on.
    pub fn bind_pose(&self) -> Result<Pose> {
        let n = self.node_hashes.len();
        let mut pose = Pose { translation: vec![[0.0; 3]; n], rotation: vec![crate::math::IDENTITY; n] };
        if let Some(bind) = self.clip("bind") {
            self.apply(bind, 0, &mut pose)?;
        }
        Ok(pose)
    }

    /// Pose of `clip` at `time_ms` (clamped to the clip), starting from `base`.
    pub fn sample(&self, clip: &Clip, time_ms: u32, base: &Pose) -> Result<Pose> {
        let mut pose = base.clone();
        self.apply(clip, time_ms.clamp(clip.start_ms(), clip.end_ms()), &mut pose)?;
        Ok(pose)
    }

    /// Like [`Anix::sample`] but without clamping the time to the clip (diagnosis: is there key data past the end?).
    pub fn sample_unclamped(&self, clip: &Clip, time_ms: u32, base: &Pose) -> Result<Pose> {
        let mut pose = base.clone();
        self.apply(clip, time_ms, &mut pose)?;
        Ok(pose)
    }

    /// Raw codec blocks of the first segment of `clip`, for reverse engineering: tag, the (channel hash,
    /// node hash) of each element, and the bytes from the block start to the end of the segment.
    pub fn codec_blocks(&self, clip: &Clip) -> Result<Vec<(String, Vec<(u32, u32)>, &[u8])>> {
        let Some(segment) = clip.segments.first() else { return Ok(Vec::new()) };
        let seg = segment.offset;
        let mut out = Vec::new();
        for (codec, tag) in clip.codecs.iter().enumerate() {
            let elements: Vec<(u32, u32)> = clip
                .elements
                .iter()
                .filter(|e| e.codec as usize == codec)
                .map(|e| (self.channel_hashes[e.channel as usize], self.node_hashes[e.node as usize]))
                .collect();
            let base = seg + u16_at(&self.a, seg + 4 + 2 * codec)? as usize;
            let end = (seg + segment.size).min(self.a.len());
            out.push((tag.clone(), elements, &self.a[base.min(end)..end]));
        }
        Ok(out)
    }

    fn apply(&self, clip: &Clip, time_ms: u32, pose: &mut Pose) -> Result<()> {
        let Some(segment) = clip
            .segments
            .iter()
            .find(|s| s.start_ms <= time_ms && time_ms <= s.end_ms)
            .or(clip.segments.last())
        else {
            return Ok(());
        };
        if segment.size == 0 {
            return Ok(());
        }
        let a = &self.a;
        let seg = segment.offset;
        let codec_count = clip.codecs.len();
        let named_count = u32_at(a, seg)? as usize;
        let named_offsets = seg + 4 + 2 * codec_count;
        let named_hashes = (named_offsets + 2 * named_count + 3) & !3;
        let mut named = HashMap::new();
        for i in 0..named_count {
            named.insert(u32_at(a, named_hashes + 4 * i)?, seg + u16_at(a, named_offsets + 2 * i)? as usize);
        }

        for (codec, tag) in clip.codecs.iter().enumerate() {
            let elements: Vec<Element> =
                clip.elements.iter().copied().filter(|e| e.codec as usize == codec).collect();
            if elements.is_empty() {
                continue;
            }
            let base = seg + u16_at(a, seg + 4 + 2 * codec)? as usize;
            let count = elements.len();
            match tag.as_str() {
                "SQ03" => self.write_rotations(&elements, &sq03(a, base, count)?, pose),
                "AQ03" => self.write_rotations(&elements, &aq03(a, base, count, time_ms)?, pose),
                "SV01" => {
                    let values = (0..count).map(|i| Ok(sv01(u32_at(a, base + 4 * i)?))).collect::<Result<Vec<_>>>()?;
                    self.write_vectors(&elements, &values, pose);
                }
                "AV01" => self.write_vectors(&elements, &av01(a, base, count, time_ms)?, pose),
                "UATR" => {
                    let lith = *named
                        .get(&name_hash("LITH"))
                        .ok_or_else(|| anyhow!("{}: UATR without its key table", clip.name))?;
                    self.write_vectors(&elements, &uatr(a, base, count, time_ms, lith)?, pose);
                }
                // Not decoded yet (STRI carries a non-transform channel on the player).
                _ => {}
            }
        }
        Ok(())
    }

    fn write_rotations(&self, elements: &[Element], values: &[Quat], pose: &mut Pose) {
        for (e, v) in elements.iter().zip(values) {
            if self.channels.get(e.channel as usize) == Some(&Channel::Rotation) {
                pose.rotation[e.node as usize] = *v;
            }
        }
    }

    fn write_vectors(&self, elements: &[Element], values: &[Vec3], pose: &mut Pose) {
        for (e, v) in elements.iter().zip(values) {
            if self.channels.get(e.channel as usize) == Some(&Channel::Position) {
                pose.translation[e.node as usize] = *v;
            }
        }
    }
}

/// 10:10:10 vector in bits 2..32, spanning +-256 cm.
fn sv01(v: u32) -> Vec3 {
    let f = |x: u32| (x as f32 / 1022.0) * 512.0 - 256.0;
    [f((v >> 2) & 0x3ff), f((v >> 12) & 0x3ff), f(v >> 22)]
}

/// Smallest-three quaternion: three 13-bit components in +-1/sqrt(2), the fourth rebuilt.
/// `index & 3` says where the rebuilt component goes (counted from w), bit 2 negates it.
fn unpack_quat(a: u64, b: u64, c: u64, index: u64) -> Quat {
    let k = |x: u64| (x as f32 / 8190.0) * std::f32::consts::SQRT_2 - std::f32::consts::FRAC_1_SQRT_2;
    let (a, b, c) = (k(a), k(b), k(c));
    let mut largest = (1.0 - a * a - b * b - c * c).max(0.0).sqrt();
    if index & 4 != 0 {
        largest = -largest;
    }
    match index & 3 {
        0 => [a, b, c, largest],
        1 => [a, b, largest, c],
        2 => [a, largest, b, c],
        _ => [largest, a, b, c],
    }
}

/// Three quaternions per 16 bytes, 42 bits each.
fn sq03(a: &[u8], base: usize, count: usize) -> Result<Vec<Quat>> {
    const M: u64 = 0x1fff;
    let mut out = Vec::with_capacity(count + 2);
    for triple in 0..count.div_ceil(3) {
        let lo = u64_at(a, base + 16 * triple)?;
        let hi = u64_at(a, base + 16 * triple + 8)?;
        out.push(unpack_quat(lo & M, (lo >> 13) & M, (lo >> 26) & M, (lo >> 39) & 7));
        out.push(unpack_quat((lo >> 42) & M, ((hi & 0xf) << 9) | (lo >> 55), (hi >> 4) & M, (hi >> 17) & 7));
        out.push(unpack_quat((hi >> 20) & M, (hi >> 33) & M, (hi >> 46) & M, (hi >> 59) & 7));
    }
    out.truncate(count);
    Ok(out)
}

/// Shared header of the animated codecs: `u32 first ms, u32 last ms, u32 key count`, then
/// `key count - 1` u16 time deltas starting at `deltas`. Returns (key index, blend factor)
/// for the key pair `index - 1`, `index`.
/// The cue track. The `STRI` block of the first segment holds, from its 0x0e-th byte, `{ u16 time (10 ms),
/// u16 text length incl. NUL, u16 offset, u16 0 }` per event; the texts themselves sit in the group
/// descriptor after the segment table, at the offset stored 4 bytes behind it.
fn read_events(
    a: &[u8],
    b: &[u8],
    group: usize,
    table_end: usize,
    codecs: &[String],
    elements: &[Element],
    segments: &[Segment],
) -> Result<Vec<Event>> {
    let Some(codec) = codecs.iter().position(|c| c == "STRI") else { return Ok(Vec::new()) };
    if !elements.iter().any(|e| e.codec as usize == codec) {
        return Ok(Vec::new());
    }
    let Some(segment) = segments.first() else { return Ok(Vec::new()) };
    let base = segment.offset + u16_at(a, segment.offset + 4 + 2 * codec)? as usize;
    let count = u16_at(a, base + 4)? as usize;
    let mut text = group + u32_at(b, table_end + 4)? as usize;
    let mut events = Vec::with_capacity(count);
    for i in 0..count {
        let record = base + 0x0e + 8 * i;
        let time = u16_at(a, record)? as u32 * 10;
        let len = u16_at(a, record + 2)? as usize;
        let bytes = b.get(text..text + len).ok_or_else(|| anyhow!("event text out of range"))?;
        events.push(Event { time_ms: time, text: bytes.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect() });
        text += len;
    }
    Ok(events)
}

fn find_keys(a: &[u8], base: usize, deltas: usize, time_ms: u32) -> Result<(usize, f32, usize)> {
    let first = u32_at(a, base)?;
    let last = u32_at(a, base + 4)?;
    let key_count = u32_at(a, base + 8)? as usize;
    ensure!(key_count >= 2, "animated block with {key_count} keys");
    let t = time_ms.clamp(first, last);
    let mut index = 1;
    let mut t0 = first;
    let mut t1 = first + u16_at(a, base + deltas)? as u32;
    while t1 < t && t1 < last {
        ensure!(index + 1 < key_count, "key index outside of valid range");
        t0 = t1;
        t1 += u16_at(a, base + deltas + 2 * index)? as u32;
        index += 1;
    }
    let factor = if t1 > t0 { (t - t0) as f32 / (t1 - t0) as f32 } else { 0.0 };
    Ok((index, factor, key_count))
}

fn aq03(a: &[u8], base: usize, count: usize, time_ms: u32) -> Result<Vec<Quat>> {
    let (index, factor, key_count) = find_keys(a, base, 0xe, time_ms)?;
    let data = (base + key_count * 2 + 0x1b) & !0xf;
    let stride = count.div_ceil(3) * 16;
    let from = sq03(a, data + stride * (index - 1), count)?;
    let to = sq03(a, data + stride * index, count)?;
    Ok(from.into_iter().zip(to).map(|(q0, q1)| nlerp(q0, q1, factor)).collect())
}

fn av01(a: &[u8], base: usize, count: usize, time_ms: u32) -> Result<Vec<Vec3>> {
    let (index, factor, key_count) = find_keys(a, base, 0x12, time_ms)?;
    let data = (base + key_count * 2 + 0x1f) & !0xf;
    let stride = ((count + 3) & !3) * 4;
    (0..count)
        .map(|i| {
            let v0 = sv01(u32_at(a, data + stride * (index - 1) + 4 * i)?);
            let v1 = sv01(u32_at(a, data + stride * index + 4 * i)?);
            Ok(vlerp(v0, v1, factor))
        })
        .collect()
}

/// Per element: a u16 offset to `{ u8 palette index, u8 weight }` per key interval, followed by
/// a palette of float vectors. Key times come from the segment's shared `LITH` block.
fn uatr(a: &[u8], base: usize, count: usize, time_ms: u32, lith: usize) -> Result<Vec<Vec3>> {
    let time_count = u32_at(a, lith)? as usize;
    if time_count < 2 {
        bail!("UATR key table with {time_count} times");
    }
    let times = (lith + 0xf) & !3;
    let time = |i: usize| u32_at(a, times + 4 * i);

    let last = time_count - 2;
    let mut interval = last;
    for i in 0..last {
        if time(i)? <= time_ms && time_ms <= time(i + 1)? {
            interval = i;
            break;
        }
    }
    let (t0, t1) = (time(interval)?, time(interval + 1)?);
    let factor = if time_ms < t1 && t1 > t0 { time_ms.saturating_sub(t0) as f32 / (t1 - t0) as f32 } else { 1.0 };

    let byte = |off: usize| a.get(off).copied().ok_or_else(|| anyhow!("UATR track out of range"));
    (0..count)
        .map(|e| {
            let track = base + u16_at(a, base + 2 * e)? as usize;
            let entry = track + 2 * interval;
            let index = byte(entry)? as usize;
            let w0 = byte(entry + 1)? as f32 / 255.0;
            // A repeated palette index means the value holds; its own weight then eases it.
            let w1 = if byte(entry)? == byte(entry + 2)? { byte(entry + 3)? as f32 / 255.0 } else { 1.0 };
            let palette = (track + 2 * time_count + 3) & !3;
            let vec = |k: usize| -> Result<Vec3> {
                let p = palette + 12 * k;
                Ok([f32_at(a, p)?, f32_at(a, p + 4)?, f32_at(a, p + 8)?])
            };
            Ok(vlerp(vec(index)?, vec(index + 1)?, w0 + (w1 - w0) * factor))
        })
        .collect()
}
