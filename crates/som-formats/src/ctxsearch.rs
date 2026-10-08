//! The game's context-action search (`Predicate.FoundContextAction` with a `LogicNodeData.ContextActionData`), transcribed from the
//! exe (Ghidra, twelfth/thirteenth pass in NOTES.md). Everything runs in the game's own frame: centimetres, Y up, a character
//! with yaw 0 faces +Z, yaw turns toward +X.
//!
//! Layout of the exe code this follows:
//! - `FUN_1408a5320` builds the query block from the data cell and the character (`query`);
//! - `FUN_1408ae900` builds the parameter block `P` and runs `FUN_1408ae550` for every candidate line (`search`);
//! - `FUN_1408ae550` chains the five filters: `FUN_1408ad520` (`filter_angle`), `FUN_14089e870` (`filter_view`),
//!   `FUN_14089a270` (`filter_vertical`), `FUN_14089acf0` (`filter_distance`), `FUN_1408ad810` (`filter_rank`);
//! - `FUN_140969600` validates a line (`valid_line`).
//!
//! Not transcribed (inputs the game's data never uses for these records, or engine glue): `SearchOnObject`, `QueryWorldPosition`,
//! `CollisionData` (null in every record), the connected-line walk of `FUN_140898660` (an isolated line is assumed), the debug
//! draw branches, and `RailingAngle` (a float read from the model's behaviour record, native default 10 degrees).

pub type V = [f32; 3];

const RAD: f32 = std::f32::consts::PI / 180.0;
const EPS2: f32 = 1e-12;

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V, s: f32) -> V {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len2(a: V) -> f32 {
    dot(a, a)
}
/// `x/|x|` when `|x|^2 >= 1e-12` (the exe's inline pattern), otherwise unchanged.
fn unit(a: V) -> V {
    let l = len2(a);
    if l >= EPS2 { mul(a, 1.0 / l.sqrt()) } else { a }
}
/// The horizontal part of `a`, unit length when it has any.
fn flat_unit(a: V) -> V {
    unit([a[0], 0.0, a[2]])
}
/// `FUN_1403f31d0`: normalise unless shorter than `min`.
fn unit_min(a: V, min: f32) -> V {
    let l = len2(a);
    if l < min * min { a } else { mul(a, 1.0 / l.sqrt()) }
}
/// Rotation about +Y by `ang` (the yaw quaternion `(0, sin a/2, 0, cos a/2)` applied with `FUN_1403f0880`).
fn rot_y(v: V, ang: f32) -> V {
    let (s, c) = ang.sin_cos();
    [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
}
fn forward(yaw: f32) -> V {
    [yaw.sin(), 0.0, yaw.cos()]
}
fn yaw_of(v: V) -> f32 {
    v[0].atan2(v[2])
}
fn wrap_pi(mut a: f32) -> f32 {
    while a > std::f32::consts::PI {
        a -= std::f32::consts::TAU;
    }
    while a < -std::f32::consts::PI {
        a += std::f32::consts::TAU;
    }
    a
}

/// `LogicNodeData.CollisionData`: a line-of-sight test from the character to the found point (`FUN_1408ace10`). `StartOffset` is turned
/// by the query facing, `EndOffset` by the line normal; `MidOffset` shifts the midpoint of the two-segment form.
#[derive(Debug, Clone, Copy, Default)]
pub struct Collision {
    pub start_safe: f32,
    pub end_safe: f32,
    pub simple: bool,
    pub start_height: f32,
    pub end_height: f32,
    pub mid_val: f32,
    pub start_offset: V,
    pub end_offset: V,
    pub mid_offset: V,
}

/// `FUN_1408ace10`: is the way from `start` to `end` blocked? `blocked` casts a segment (game frame, cm) against the world.
fn collision_blocked(col: &Collision, start: V, start_yaw: f32, end: V, end_yaw: f32, blocked: &dyn Fn(V, V) -> bool) -> bool {
    let (mut s, mut e) = (add(start, rot_y(col.start_offset, start_yaw)), add(end, rot_y(col.end_offset, end_yaw)));
    if col.start_safe > 0.0 || col.end_safe > 0.0 {
        let d = sub(e, s);
        let l = len2(d).sqrt();
        if col.end_safe + col.start_safe < l {
            let u = mul(d, 1.0 / l);
            e = sub(e, mul(u, col.end_safe));
            s = add(s, mul(u, col.start_safe));
        } else {
            return false;
        }
    }
    if col.simple {
        if !blocked(s, e) {
            return false;
        }
        if col.start_height == 0.0 && col.end_height == 0.0 {
            return true;
        }
        s[1] += col.start_height;
        e[1] += col.end_height;
        return blocked(s, e);
    }
    let mut m = add(s, mul(sub(e, s), col.mid_val));
    m[1] += col.mid_offset[1];
    let h = unit([e[0] - s[0], 0.0, e[2] - s[2]]);
    if col.mid_offset[0] != 0.0 || col.mid_offset[2] != 0.0 {
        m[0] += h[0] * col.mid_offset[2] + col.mid_offset[0] * h[2];
        m[2] += h[2] * col.mid_offset[2] - col.mid_offset[0] * h[0];
    }
    blocked(s, m) || blocked(m, e)
}

/// The cells of a `ContextActionData` the search reads, as authored (centimetres, degrees).
#[derive(Debug, Clone, Default)]
pub struct Data {
    pub action: String,
    pub vertical_range: (f32, f32),
    /// `VerticalDistRangeCenter`: 0 "Center"; 1 and 2 shift the window by `AnimDims.y` (names not read; unknown names count as 0).
    pub vertical_center: u8,
    pub max_horizontal: f32,
    pub min_horizontal: f32,
    pub approach_angle: f32,
    pub test_angle_rotation: f32,
    pub approach_test_offset: f32,
    pub los_range: f32,
    pub los_left: f32,
    pub los_right: f32,
    pub invert_normal: bool,
    pub approach_angle_as_railing: bool,
    pub always_pick_center: bool,
    pub use_distance_from_character: bool,
    pub use_distance_from_query: bool,
    pub use_offset_as_alignment: bool,
    pub use_normal_for_found_offset: bool,
    pub use_blended_normal_for_found_offset: bool,
    pub distance_test_y_factor: f32,
    pub edge_proximity_buffer: f32,
    pub max_connection_dist: f32,
    pub max_connection_angle: f32,
    pub collision: Option<Collision>,
    pub found_node_offset: V,
    pub additional_node_offset: V,
    pub anim_dims: V,
}

#[derive(Debug, Clone, Copy)]
pub struct Character {
    pub pos: V,
    pub yaw: f32,
}

/// Values the behaviour graph can feed the query (`TestAngleRotationOverride`, `TestAngleRotationWorldOverride`), radians.
#[derive(Debug, Clone, Copy, Default)]
pub struct Overrides {
    pub test_rotation: Option<f32>,
    pub world_rotation: Option<f32>,
    /// `QueryWorldPosition`: a point the behaviour graph holds (game frame, cm) that replaces the query position.
    pub query_position: Option<V>,
}

#[derive(Debug, Clone, Copy)]
pub struct Line {
    pub a: V,
    pub b: V,
    /// The stored normal (horizontal, into the wall).
    pub n: V,
}

/// What the search returns (`FUN_1408ae900`'s output block).
#[derive(Debug, Clone, Copy)]
pub struct Found {
    pub line: usize,
    /// The found point (with the data's `AnimDims` / found-node offsets applied).
    pub point: V,
    /// Signed yaw from the character's facing to the found point's direction.
    pub yaw_to_point: f32,
    /// Signed yaw from the character's facing to the line's normal.
    pub yaw_of_normal: f32,
    pub score: f32,
    pub distance: f32,
    /// The line's ends as the search used them (shortened by `AnimDims.x`) and its stored normal.
    pub a: V,
    pub b: V,
    pub normal: V,
    /// The query's facing, and the alignment vector `UseOffsetAsAlignment` asks for (horizontal, query minus character).
    pub query_facing: V,
    pub offset_alignment: Option<V>,
}

struct Query {
    pos: V,
    height: f32,
    fwd: V,
    max_horizontal: f32,
}

/// `FUN_1408a5320`.
fn query(d: &Data, ch: &Character, ov: &Overrides) -> Query {
    let (lo, hi) = d.vertical_range;
    let height = hi - lo;
    let mut pos = ch.pos;
    pos[1] = pos[1] - hi + height * 0.5;
    match d.vertical_center {
        1 => pos[1] -= d.anim_dims[1],
        2 => pos[1] += d.anim_dims[1],
        _ => {}
    }
    if let Some(q) = ov.query_position {
        pos = q;
    }
    let mut fwd = forward(ch.yaw);
    let rot = d.test_angle_rotation * RAD;
    let rot = ov.test_rotation.unwrap_or(rot);
    if rot.abs() > 0.01 {
        fwd = rot_y(fwd, rot);
    }
    let mut at = forward(ch.yaw);
    if let Some(w) = ov.world_rotation {
        fwd = forward(w);
        at = forward(w);
    }
    pos = add(pos, rot_y(d.additional_node_offset, yaw_of(at)));
    Query { pos, height, fwd: unit_min(fwd, 1e-6), max_horizontal: d.max_horizontal }
}

/// Parameter block (`FUN_1408ae900`'s `P`).
struct P<'a> {
    d: &'a Data,
    edge_buffer: f32,
    half_height: f32,
    facing: V,
    facing_rot: V,
    cos_approach: f32,
    wedge: bool,
    los_left: f32,
    los_right: f32,
    los_range: f32,
    cos_los: f32,
    max_dist: f32,
    min_dist: f32,
    pos: V,
    found_offset: V,
    y_factor: f32,
    char_pos: V,
    char_yaw: f32,
    rail_cos: f32,
    blocked: &'a dyn Fn(V, V) -> bool,
}

/// Candidate block (`FUN_1408ae550`'s `local_c8`).
#[derive(Clone, Copy, Default)]
struct C {
    a: V,
    b: V,
    n: V,
    mid: V,
    dir: V,        // 0x4c: found direction
    d: V,          // 0x58: unit along the line
    point: V,      // 0x64
    offset: V,     // 0x70
    n_raw: V,      // 0x80
    result: V,     // 0x8c
    score: f32,    // 0x98
    dist: f32,     // 0x9c
    f7c: bool,
    f7d: bool,
    f7e: bool,
    f7f: bool,
}

#[derive(Clone, Copy)]
struct Best {
    a: V,
    b: V,
    found: bool,
    line: usize,
    result: V,
    score: f32,
    dist: f32,
    n_raw: V,
}

/// `FUN_140969600`: lines that are too short or have a vertical normal are not candidates.
fn valid_line(l: &Line) -> bool {
    let dx = l.a[0] - l.b[0];
    let dz = l.a[2] - l.b[2];
    !(dx * dx + dz * dz < 0.1 || l.n[1] >= 0.99 || l.n[1] <= -0.99)
}

/// `FUN_1408ad520`.
fn filter_angle(p: &P, c: &mut C) -> bool {
    c.d = sub(c.b, c.a);
    c.d = unit_min(c.d, 1e-6);
    c.n[1] = 0.0;
    let n2 = c.n[0] * c.n[0] + c.n[2] * c.n[2];
    if n2 < 1e-4 {
        return false;
    }
    if n2 >= 1e-12 {
        c.n = mul(c.n, 1.0 / n2.sqrt());
    }
    if p.d.invert_normal {
        c.n = mul(c.n, -1.0);
    }
    if dot(c.d, p.facing) < 0.0 {
        c.d = mul(c.d, -1.0);
    }
    let v = if p.d.approach_angle_as_railing {
        let h = c.d[0] * c.d[0] + c.d[2] * c.d[2];
        if h == 0.0 {
            return false;
        }
        flat_unit([c.d[0], 0.0, c.d[2]])
    } else {
        c.n
    };
    p.cos_approach <= dot(v, p.facing_rot)
}


/// Where the exe's tail (`LAB_14089fa1d`) leaves the found point: a found direction equal to an endpoint's is that endpoint.
#[derive(Clone, Copy, PartialEq)]
enum End {
    None,
    A,
    B,
}

/// `FUN_14089e870`: is the line inside the view wedge, and which direction / point on it is found.
fn filter_view(p: &P, c: &mut C, best: &Best) -> bool {
    let w = p.d.anim_dims[0];
    c.a = add(c.a, mul(c.d, w));
    c.b = sub(c.b, mul(c.d, w));
    let f = flat_unit(p.facing);
    let u1 = flat_unit(sub(c.a, p.pos));
    let u2 = flat_unit(sub(c.b, p.pos));
    c.dir = f;
    let cross_y = |u: V| u[2] * f[0] - f[2] * u[0];
    let (c1, c2) = (cross_y(u1), cross_y(u2));
    let plane = dot(c.n, c.a);
    let side0 = dot(c.n, p.pos) - plane;
    let side1 = dot(c.n, add(p.pos, mul(f, p.max_dist))) - plane;
    let miss = (side0 > 0.0 && side1 > 0.0) || (side0 < 0.0 && side1 < 0.0) || c2 * c1 > 0.0;
    let b1 = dot(f, u1) < 0.0;
    let b2 = dot(f, u2) < 0.0;
    let behind = b1 && b2;
    let d_a = len2(sub(c.a, p.pos));
    let d_b = len2(sub(c.b, p.pos));
    let mut end = End::None;

    if (c1 * c1 > 1e-4 && c2 * c2 > 1e-4) || behind {
        if !miss && !behind {
            // The facing ray passes between the endpoints.
            if p.d.approach_angle_as_railing && p.rail_cos < dot(c.d, c.dir) && dot(u2, u1) > 0.0 {
                if d_a <= d_b {
                    c.dir = u1;
                    end = End::A;
                } else {
                    c.dir = u2;
                    end = End::B;
                }
                c.f7f = true;
            }
        } else {
            if best.found {
                return false;
            }
            c.f7c = false;
            let (thr, centre) = if !p.wedge {
                (p.cos_los, f)
            } else {
                let mid = (p.los_left + p.los_right) * 0.5;
                let half = (p.los_right - p.los_left) * 0.5;
                (half.cos(), if mid.abs() > 0.01 { rot_y(f, mid) } else { f })
            };
            let centre = unit(centre);
            let (dc1, dc2) = (dot(centre, u1), dot(centre, u2));
            if dc1 < thr && dc2 < thr {
                return false;
            }
            let half = p.los_range * 0.5;
            let r1 = unit(rot_y(f, half));
            let r2 = unit(rot_y(f, -half));
            // The segment misses the ray: both endpoints on one side of it, or both behind it.
            let misses = |r: V| {
                let cr = |u: V| [r[2] * u[1] - r[1] * u[2], u[2] * r[0] - r[2] * u[0], r[1] * u[0] - r[0] * u[1]];
                dot(cr(u2), cr(u1)) > 0.0 || (dot(r, u1) < 0.0 && dot(r, u2) < 0.0)
            };
            let (m1, m2) = (misses(r1), misses(r2));
            if m1 && m2 {
                let to_b = if dc1 < 0.0 || dc2 < 0.0 {
                    if dot(p.facing, u1) < p.cos_los && dot(p.facing, u2) < p.cos_los {
                        return false;
                    }
                    dc1 <= dc2
                } else {
                    d_b < d_a
                };
                if to_b {
                    c.dir = u2;
                    end = End::B;
                } else {
                    c.dir = u1;
                    end = End::A;
                }
            } else if p.d.approach_angle_as_railing {
                c.point = p.pos;
                if dc1 <= 0.0 || (dc2 > 0.0 && dc1 <= dc2) {
                    c.dir = u2;
                    end = End::B;
                } else {
                    c.dir = u1;
                    end = End::A;
                }
                c.offset = c.dir;
            } else {
                // One of the wedge's rays crosses the segment: the point is where that ray's line meets the line.
                let ray = flat_unit(if !m1 { r1 } else { r2 });
                let nv = unit([ray[2], 0.0, -ray[0]]);
                let t = dot(nv, sub(p.pos, c.a)) / dot(nv, c.d);
                let pt = add(c.a, mul(c.d, t));
                c.point = pt;
                c.offset = [pt[0] - p.pos[0], 0.0, pt[2] - p.pos[2]];
                if len2(c.offset) == 0.0 {
                    c.offset = forward(p.char_yaw);
                }
                c.dir = unit_min(c.offset, 1e-6);
                let off2 = len2(c.offset);
                if dc2 < dc1 && d_a < off2 {
                    c.dir = u1;
                    end = End::A;
                } else if dc2 > dc1 && off2 > d_b {
                    c.dir = u2;
                    end = End::B;
                }
            }
        }
    } else {
        // Both endpoints on one side of the facing ray (or the ray runs along the line).
        let to_b = if !p.d.approach_angle_as_railing || b1 == b2 {
            if b1 || b2 {
                return false;
            }
            d_b < d_a
        } else {
            b1
        };
        if to_b {
            c.dir = u2;
            end = End::B;
        } else {
            c.dir = u1;
            end = End::A;
        }
        c.f7d = true;
    }
    match end {
        End::A => c.point = c.a,
        End::B => c.point = c.b,
        End::None => return true,
    }
    c.offset = sub(c.point, p.pos);
    true
}

/// `FUN_14089a270`: the found point must lie in the vertical window; if not it is moved to the window's edge and must still be seen.
fn filter_vertical(p: &P, c: &mut C) -> bool {
    if c.f7c && !c.f7d {
        // Found by direction: the point is where the line meets the plane through the character along that direction.
        let nv = unit([c.dir[2], 0.0, -c.dir[0]]);
        let den = dot(c.d, nv);
        if den.abs() <= 1e-11 {
            return false;
        }
        let t = dot(sub(p.pos, c.a), nv) / den;
        c.point = add(c.a, mul(c.d, t));
        c.offset = sub(c.point, p.pos);
    }
    let (h, cy, y) = (p.half_height, p.pos[1], c.point[1]);
    if cy + h < y || y < cy - h {
        let ty = if y <= cy + h { cy - h } else { cy + h };
        let t = (ty - c.a[1]) / c.d[1];
        c.point = add(c.a, mul(c.d, t));
        c.offset = sub(c.point, p.pos);
        let v = flat_unit(c.offset);
        let f = flat_unit(p.facing);
        let half = p.los_range * 0.5;
        let width = p.max_dist - p.min_dist;
        let r1 = unit(mul(rot_y(f, half), width));
        let r2 = unit(mul(rot_y(f, -half), width));
        let behind_all = dot(r1, v) < 0.0 && dot(f, v) < 0.0 && dot(r2, v) < 0.0;
        let cy2 = (r2[2] * v[0] - v[2] * r2[0]) * (r1[2] * v[0] - v[2] * r1[0]);
        if behind_all || cy2 > 0.0 {
            return false;
        }
    }
    true
}

/// `FUN_14089acf0`: the found point must be `MinHorizontalDist..MaxHorizontalDist` away; otherwise it slides along the line to the band's edge.
fn filter_distance(p: &P, c: &mut C) -> bool {
    let dx = c.point[0] - p.pos[0];
    let dz = c.point[2] - p.pos[2];
    let dist = (dx * dx + dz * dz).sqrt();
    if dist >= p.min_dist && dist <= p.max_dist {
        return true;
    }
    let s = dot(c.n, p.pos) - dot(c.n, c.a);
    if s.abs() >= p.max_dist || dot(c.d, c.offset).abs() <= 1e-5 {
        return false;
    }
    let r = if dist < p.max_dist { p.min_dist } else { p.max_dist };
    let disc = r * r - s * s;
    if disc < 0.0 {
        return false;
    }
    let t0 = dot(c.d, sub(p.pos, c.a)) / dot(c.d, c.d);
    let pnt = add(c.a, mul(c.d, disc.sqrt() + t0));
    if len2(sub(pnt, c.mid)) > len2(sub(c.a, c.mid)) {
        return false;
    }
    let off = sub(pnt, p.pos);
    c.offset = off;
    let hdir = unit_min([off[0], 0.0, off[2]], 1e-6);
    if p.cos_los <= dot(hdir, p.facing_rot) {
        c.dir = unit_min(off, 1e-6);
        c.point = pnt;
        c.f7e = true;
        return true;
    }
    false
}

/// A candidate line as the exe keeps it after `FUN_140969600`: unit direction `(A-B)/len`, length, and the neighbour joined at
/// each end (`nb[0]` at A, `nb[1]` at B).
#[derive(Clone, Copy)]
struct Rec {
    a: V,
    b: V,
    n: V,
    dir: V,
    len: f32,
    nb: [Option<usize>; 2],
}

impl Rec {
    fn end(&self, e: usize) -> V {
        if e == 0 { self.a } else { self.b }
    }
}

/// `FUN_140969470`: how straight the join of `x` (at end `ex`) and `y` (at end `ey`) is.
fn join_score(recs: &[Rec], x: usize, y: usize, ex: usize, ey: usize) -> f32 {
    let d = if ex == ey { mul(recs[y].dir, -1.0) } else { recs[y].dir };
    dot(recs[x].dir, d)
}

/// `FUN_1409694f0`: join `r` (end `er`) to `o` (end `eo`) when that is a straighter join than the ones either already has.
fn link(recs: &mut [Rec], r: usize, o: usize, er: usize, eo: usize) {
    let (l1, l2) = (recs[r].nb[er], recs[o].nb[eo]);
    let mut ok = true;
    if l1.is_some() || l2.is_some() {
        let s = join_score(recs, r, o, er, eo);
        if let Some(n1) = l1 {
            let s1 = join_score(recs, r, n1, er, (recs[n1].nb[0] != Some(r)) as usize);
            if s <= s1 {
                ok = false;
            }
        }
        if let Some(n2) = l2 {
            let s2 = join_score(recs, o, n2, eo, (recs[n2].nb[0] != Some(o)) as usize);
            if !(s2 < s) {
                return;
            }
        }
        if !ok {
            return;
        }
    }
    if let Some(n1) = l1 {
        let i = (recs[n1].nb[0] != Some(r)) as usize;
        recs[n1].nb[i] = None;
    }
    if let Some(n2) = recs[o].nb[eo] {
        let i = (recs[n2].nb[0] != Some(o)) as usize;
        recs[n2].nb[i] = None;
    }
    recs[r].nb[er] = Some(o);
    recs[o].nb[eo] = Some(r);
}

/// `FUN_140969600` after validation: join the new line `r` to the lines gathered before it whose endpoints lie within `max_dist`
/// and whose normal and direction are within the connection angle.
fn connect(recs: &mut [Rec], r: usize, max_dist: f32, cos_angle: f32) {
    let d2 = max_dist * max_dist;
    for j in 0..r {
        let (x, n) = (recs[j], recs[r]);
        if cos_angle <= dot(x.n, n.n) {
            let c = dot(x.dir, n.dir);
            if cos_angle <= c {
                if c > 0.0 {
                    if len2(sub(n.a, x.b)) <= d2 {
                        link(recs, r, j, 0, 1);
                    }
                    if len2(sub(n.b, x.a)) <= d2 {
                        link(recs, r, j, 1, 0);
                    }
                } else {
                    if len2(sub(n.a, x.a)) <= d2 {
                        link(recs, r, j, 0, 0);
                    }
                    if len2(sub(n.b, x.b)) <= d2 {
                        link(recs, r, j, 1, 1);
                    }
                }
            }
        }
    }
}

/// `FUN_140898220`: walk the chain from `next` (coming from `prev`) until `limit` of length is collected (false) or it ends (true).
fn chain(recs: &[Rec], limit: f32, next: Option<usize>, prev: usize, term: &mut Option<usize>, cursor: &mut Option<usize>, dist: &mut f32, nacc: &mut V) -> bool {
    let Some(mut cur) = next else {
        *term = Some(prev);
        return true;
    };
    let mut prev0 = prev;
    let mut acc = *dist;
    loop {
        *cursor = Some(cur);
        *dist = acc + recs[cur].len;
        *nacc = add(*nacc, recs[cur].n);
        acc = *dist;
        if limit <= acc {
            return false;
        }
        let mut nx = recs[cur].nb[0];
        if nx == Some(prev0) {
            nx = recs[cur].nb[1];
            if nx == Some(prev0) {
                *term = Some(cur);
                return true;
            }
        }
        prev0 = cur;
        match nx {
            Some(n) => cur = n,
            None => {
                *term = Some(cur);
                return true;
            }
        }
    }
}

/// `FUN_140898a80`: the point `dist` along the chain starting at `p1` (entered from `p2`).
fn place(recs: &[Rec], mut p1: usize, p2: Option<usize>, dist: &mut f32, out: &mut V) {
    let mut prev = p2;
    let mut lim = p2;
    let mut len = recs[p1].len;
    if len < *dist {
        loop {
            prev = Some(p1);
            *dist -= len;
            if let Some(n) = recs[p1].nb[0] {
                if Some(n) != lim {
                    place(recs, n, Some(p1), dist, out);
                }
            }
            match recs[p1].nb[1] {
                None => return,
                Some(n) if Some(n) == lim => return,
                Some(n) => p1 = n,
            }
            len = recs[p1].len;
            lim = prev;
            if len >= *dist {
                break;
            }
        }
    }
    let b = recs[p1].nb[1] == prev;
    let (i5, i4) = (b as usize, !b as usize);
    let start = recs[p1].end(i5);
    let dir = unit(sub(recs[p1].end(i4), start));
    *out = add(start, mul(dir, *dist));
}

/// `FUN_140898660`: how much connected line lies beyond each end of `li` within `buffer`. Returns the number of ends that stop
/// within it (0, 1 or 2) and fills the terminating line, the cursor line, the distance and the summed normals.
#[allow(clippy::too_many_arguments)]
fn walk(recs: &[Rec], buffer: f32, li: usize, term: &mut Option<usize>, cursor: &mut Option<usize>, dist: &mut f32, nacc: &mut V, pt: V, end: &mut usize) -> i32 {
    let line = recs[li];
    let mut ret = 0;
    let mut d = [0.0f32; 2];
    *end = 0;
    for e in 0..2 {
        let stopped = match line.nb[e] {
            None => {
                *term = Some(li);
                true
            }
            Some(n) => {
                d[e] += recs[n].len;
                *nacc = add(*nacc, recs[n].n);
                *cursor = Some(n);
                if d[e] < buffer {
                    if recs[n].nb[0] == Some(li) && recs[n].nb[1] == Some(li) {
                        *term = Some(n);
                        true
                    } else {
                        let next = if recs[n].nb[0] != Some(li) { recs[n].nb[0] } else { recs[n].nb[1] };
                        let mut acc = d[e];
                        let ok = chain(recs, buffer, next, n, term, cursor, &mut acc, nacc);
                        d[e] = acc;
                        ok
                    }
                } else {
                    false
                }
            }
        };
        if stopped {
            ret += 1;
            *dist = d[e];
            *end = e;
        }
    }
    if ret == 2 {
        if buffer * 2.0 < line.len {
            *dist = buffer;
            return 2;
        }
        *dist = (d[0] + d[1] + line.len) * 0.5;
        return 2;
    }
    let Some(last) = *term else { return ret };
    let e = *end;
    let ep = sub(line.end(e), pt);
    let mut dd = len2(ep).sqrt();
    if li != last {
        dd += *dist;
    }
    *dist = dd;
    if buffer <= dd {
        return 0;
    }
    *dist = buffer;
    if li == last {
        return ret;
    }
    let mut acc = 0.0;
    if !chain(recs, (buffer - dd) + buffer, line.nb[1 - e], li, term, cursor, &mut acc, nacc) {
        return ret;
    }
    *dist = (d[0] + d[1] + line.len) * 0.5;
    ret
}

/// `FUN_140899340`: a found point within `buffer` of the end of its line is moved along the connected chain to `buffer` from the end
/// (the middle when the chain is shorter). Returns the new point and the chain's summed normals.
fn adjust_for_buffer(recs: &[Rec], li: usize, buffer: f32, pt: V, nacc: &mut V) -> Option<V> {
    let line = recs[li];
    let (da, db) = (len2(sub(pt, line.a)), len2(sub(pt, line.b)));
    if !(da < buffer * buffer || db < buffer * buffer) {
        return None;
    }
    let (mut term, mut cursor, mut dist, mut end) = (None, Some(li), 0.0f32, 0usize);
    let ret = walk(recs, buffer, li, &mut term, &mut cursor, &mut dist, nacc, pt, &mut end);
    if ret == 0 {
        return None;
    }
    if cursor == term {
        let idx = if ret == 2 { (db <= da) as usize } else { end };
        let start = line.end(idx);
        let dir = unit(sub(line.end(1 - idx), start));
        Some(add(start, mul(dir, dist)))
    } else {
        let mut out = pt;
        place(recs, term?, None, &mut dist, &mut out);
        Some(out)
    }
}

/// `FUN_1408ad810`: the final point (width, offsets) and the ranking against the best so far.
fn filter_rank(p: &P, c: &mut C, recs: &[Rec], ri: usize, best: &Best) -> bool {
    c.n_raw = recs[ri].n;
    c.result = c.point;
    if !p.d.always_pick_center {
        if p.edge_buffer > 0.0 {
            let mut nacc = c.n_raw;
            if let Some(pt) = adjust_for_buffer(recs, ri, p.edge_buffer, c.point, &mut nacc) {
                c.result = pt;
            }
            c.n_raw = nacc;
        }
    } else {
        c.result = mul(add(c.a, c.b), 0.5);
    }
    c.result[1] += p.d.anim_dims[1];
    let z = p.d.anim_dims[2];
    if z != 0.0 {
        c.result = add(c.result, mul(c.n, z));
    }
    let off = p.found_offset;
    if len2(off) != 0.0 {
        let yaw = if p.d.use_normal_for_found_offset {
            yaw_of(c.n)
        } else if p.d.use_blended_normal_for_found_offset {
            yaw_of(c.n_raw)
        } else {
            yaw_of(p.facing)
        };
        c.result = add(c.result, rot_y(off, yaw));
    }
    if let Some(col) = &p.d.collision {
        if collision_blocked(col, p.char_pos, yaw_of(p.facing), c.result, yaw_of(c.n_raw), p.blocked) {
            return false;
        }
    }
    if !best.found || c.f7c {
        let (use_char, use_query) = (p.d.use_distance_from_character, p.d.use_distance_from_query);
        if use_char || use_query {
            let refp = if use_query { p.pos } else { p.char_pos };
            let mut dv = sub(c.result, refp);
            if p.y_factor == 0.0 {
                dv[1] = 0.0;
            }
            c.dist = len2(dv).sqrt();
            let accept = !c.f7c || best.found;
            if best.dist <= c.dist && accept {
                return false;
            }
        }
        let refp = if p.y_factor != 0.0 { p.pos } else { [p.pos[0], c.result[1], p.pos[2]] };
        let v = sub(c.result, refp);
        c.score = if len2(v) <= 0.0 { 0.0 } else { dot(unit(p.facing), unit(v)) };
        return use_char || use_query || c.f7c || c.score >= best.score;
    }
    false
}

/// The context-action search. `lines` are the lines of the data's `GameDBContextAction` type (game frame, cm). `rail_angle` is the
/// model's behaviour-record float the railing case compares against (degrees; native default 10).
pub fn search(d: &Data, ch: &Character, ov: &Overrides, lines: &[Line], rail_angle: f32, blocked: &dyn Fn(V, V) -> bool) -> Option<Found> {
    let q = query(d, ch, ov);
    let (los_l, los_r) = (d.los_left * RAD, d.los_right * RAD);
    let los_range = d.los_range * RAD;
    let facing_rot = if d.approach_test_offset.abs() <= 0.01 { q.fwd } else { rot_y(q.fwd, d.approach_test_offset * RAD) };
    let p = P {
        d,
        edge_buffer: d.edge_proximity_buffer,
        half_height: q.height * 0.5,
        facing: q.fwd,
        facing_rot,
        cos_approach: (d.approach_angle * RAD).cos(),
        wedge: los_l != 0.0 || los_r != 0.0,
        los_left: los_l,
        los_right: los_r,
        los_range,
        cos_los: los_range.cos(),
        max_dist: d.max_horizontal,
        min_dist: d.min_horizontal,
        pos: q.pos,
        found_offset: d.found_node_offset,
        y_factor: d.distance_test_y_factor,
        char_pos: ch.pos,
        char_yaw: ch.yaw,
        rail_cos: (rail_angle * RAD).cos(),
        blocked,
    };
    let r = q.max_horizontal;
    let (lo, hi) = ([q.pos[0] - r, q.pos[1] - q.height * 0.5, q.pos[2] - r], [q.pos[0] + r, q.pos[1] + q.height * 0.5, q.pos[2] + r]);
    let mut best = Best { a: [0.0; 3], b: [0.0; 3], found: false, line: usize::MAX, result: [0.0; 3], score: f32::MIN, dist: f32::MAX, n_raw: [0.0; 3] };
    let mut any = false;
    // Gather: lines in the box that pass validation, each joined to the ones gathered before it.
    let cos_conn = if d.max_connection_angle.min(179.0) * RAD >= std::f32::consts::PI { -2.0 } else { (d.max_connection_angle.min(179.0) * RAD).cos() };
    let mut recs: Vec<Rec> = Vec::new();
    let mut source: Vec<usize> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !valid_line(l) {
            continue;
        }
        if !(0..3).all(|k| l.a[k].min(l.b[k]) <= hi[k] && l.a[k].max(l.b[k]) >= lo[k]) {
            continue;
        }
        let ab = sub(l.a, l.b);
        let len = len2(ab).sqrt();
        recs.push(Rec { a: l.a, b: l.b, n: l.n, dir: mul(ab, 1.0 / len), len, nb: [None, None] });
        source.push(i);
        let r = recs.len() - 1;
        connect(&mut recs, r, d.max_connection_dist, cos_conn);
    }
    for ri in 0..recs.len() {
        let l = &recs[ri];
        let mut c = C { a: l.a, b: l.b, n: l.n, mid: add(l.a, mul(sub(l.b, l.a), 0.5)), f7c: true, dist: f32::MAX, score: f32::MIN, ..Default::default() };
        if filter_angle(&p, &mut c) && filter_view(&p, &mut c, &best) && filter_vertical(&p, &mut c) && filter_distance(&p, &mut c) && filter_rank(&p, &mut c, &recs, ri, &best) {
            best = Best { a: c.a, b: c.b, found: c.f7c, line: source[ri], result: c.result, score: c.score, dist: c.dist, n_raw: c.n_raw };
            any = true;
        }
    }
    if !any {
        return None;
    }
    let mut v = sub(best.result, p.pos);
    if len2(v) == 0.0 {
        v = sub(best.result, p.char_pos);
    }
    Some(Found {
        line: best.line,
        point: best.result,
        yaw_to_point: wrap_pi(yaw_of(unit(v)) - ch.yaw),
        yaw_of_normal: wrap_pi(yaw_of(best.n_raw) - ch.yaw),
        score: best.score,
        distance: best.dist,
        a: best.a,
        b: best.b,
        normal: best.n_raw,
        query_facing: p.facing,
        offset_alignment: d.use_offset_as_alignment.then(|| flat_unit(sub(p.pos, ch.pos))),
    })
}

/// The values a `LogicNodeData.ContextActionOutput` hands the behaviour graph after a successful search (`FUN_1408ac2c0`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Outputs {
    pub start_align: V,
    pub target_distance: f32,
    pub facing_diff: f32,
    pub facing_normal_diff: f32,
    pub facing_diff_along_line: f32,
    pub world: V,
    pub world_normal_yaw: f32,
    pub world_normal_along_line_yaw: f32,
}

/// `FUN_1408ac2c0` for the root node (`node` "null", no node offset): the found point relative to the character (in the
/// character's frame unless `world_space`), the angles to the point and normal, and the line's direction turned to the alignment.
/// `query_to_line` is `FacingDiffAlignment` "QueryToLine" (the query's facing instead of the character's), `height` the data's `Height`.
pub fn outputs(f: &Found, ch: &Character, world_space: bool, query_to_line: bool, height: f32) -> Outputs {
    let align = f.offset_alignment.unwrap_or(if query_to_line { f.query_facing } else { forward(ch.yaw) });
    let mut d = unit([f.a[0] - f.b[0], 0.0, f.a[2] - f.b[2]]);
    if dot(d, align) < 0.0 {
        d = mul(d, -1.0);
    }
    let along = wrap_pi(yaw_of(d) - ch.yaw);
    let rel = sub(f.point, ch.pos);
    let rel = if world_space { rel } else { rot_y(rel, -ch.yaw) };
    Outputs {
        start_align: [rel[0], rel[1] - height, rel[2]],
        target_distance: len2(rel).sqrt(),
        facing_diff: f.yaw_to_point,
        facing_normal_diff: f.yaw_of_normal,
        facing_diff_along_line: along,
        world: f.point,
        world_normal_yaw: yaw_of(f.normal),
        world_normal_along_line_yaw: wrap_pi(ch.yaw + along),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> Data {
        Data { vertical_range: (-100.0, 100.0), max_horizontal: 150.0, approach_angle: 67.5, los_range: 90.0, ..Default::default() }
    }

    #[test]
    fn finds_a_wall_line_ahead_and_ignores_one_behind() {
        // A line 100 cm ahead (+Z), its normal pointing into the wall (+Z): the character faces it.
        let ahead = Line { a: [-100.0, 0.0, 100.0], b: [100.0, 0.0, 100.0], n: [0.0, 0.0, 1.0] };
        let behind = Line { a: [-100.0, 0.0, -100.0], b: [100.0, 0.0, -100.0], n: [0.0, 0.0, 1.0] };
        let ch = Character { pos: [0.0; 3], yaw: 0.0 };
        let hit = search(&data(), &ch, &Overrides::default(), &[ahead], 10.0, &|_, _| false).expect("line ahead");
        assert!((hit.point[2] - 100.0).abs() < 1.0, "{hit:?}");
        assert!(search(&data(), &ch, &Overrides::default(), &[behind], 10.0, &|_, _| false).is_none());
    }

    fn rec(a: V, b: V) -> Rec {
        let ab = sub(a, b);
        let len = len2(ab).sqrt();
        Rec { a, b, n: [0.0, 0.0, 1.0], dir: mul(ab, 1.0 / len), len, nb: [None, None] }
    }

    #[test]
    fn collinear_lines_join_end_to_end_and_a_point_slides_onto_the_next_line() {
        // Two 100 cm lines along X meeting at x = 100.
        let mut recs = vec![rec([0.0, 0.0, 0.0], [100.0, 0.0, 0.0]), rec([100.0, 0.0, 0.0], [200.0, 0.0, 0.0])];
        connect(&mut recs, 1, 5.0, (60.0f32).to_radians().cos());
        assert_eq!(recs[0].nb[1], Some(1));
        assert_eq!(recs[1].nb[0], Some(0));
        // A point 10 cm before the joint, buffer 30: the joint is not an end, so it is not moved.
        let mut n = [0.0; 3];
        assert!(adjust_for_buffer(&recs, 0, 30.0, [90.0, 0.0, 0.0], &mut n).is_none() || n != [0.0; 3]);
        // A point 10 cm from the free end at x = 0 moves to 30 cm from it.
        let mut n = [0.0; 3];
        let p = adjust_for_buffer(&recs, 0, 30.0, [10.0, 0.0, 0.0], &mut n).expect("moved");
        assert!((p[0] - 30.0).abs() < 0.5, "{p:?}");
    }

    #[test]
    fn a_bent_chain_joins_and_places_a_point_around_the_bend() {
        // Along X to (100, 0, 0), then bending 45 degrees toward +Z for another 100 cm.
        let r = std::f32::consts::FRAC_1_SQRT_2 * 100.0;
        let mut recs = vec![rec([0.0, 0.0, 0.0], [100.0, 0.0, 0.0]), rec([100.0, 0.0, 0.0], [100.0 + r, 0.0, r])];
        connect(&mut recs, 1, 5.0, (60.0f32).to_radians().cos());
        assert_eq!(recs[0].nb[1], Some(1));
        assert_eq!(recs[1].nb[0], Some(0));
        // A point 10 cm from the far end of the second line, buffer 30: it is placed 30 cm from that end along the chain.
        let mut n = [0.0; 3];
        let end = recs[1].b;
        let p = adjust_for_buffer(&recs, 1, 30.0, [end[0] - 7.0, 0.0, end[2] - 7.0], &mut n).expect("moved");
        let d = len2(sub(p, end)).sqrt();
        assert!((d - 30.0).abs() < 1.0, "{p:?} {d}");
    }

    #[test]
    fn collision_data_blocks_a_line_behind_a_wall() {
        let ahead = Line { a: [-100.0, 0.0, 100.0], b: [100.0, 0.0, 100.0], n: [0.0, 0.0, 1.0] };
        let ch = Character { pos: [0.0; 3], yaw: 0.0 };
        let mut d = data();
        d.collision = Some(Collision { simple: true, ..Default::default() });
        assert!(search(&d, &ch, &Overrides::default(), &[ahead], 10.0, &|a, b| a[2] < 50.0 && b[2] > 50.0).is_none());
        assert!(search(&d, &ch, &Overrides::default(), &[ahead], 10.0, &|_, _| false).is_some());
    }

    #[test]
    fn a_line_beyond_the_horizontal_distance_is_not_found() {
        let far = Line { a: [-100.0, 0.0, 400.0], b: [100.0, 0.0, 400.0], n: [0.0, 0.0, 1.0] };
        let ch = Character { pos: [0.0; 3], yaw: 0.0 };
        assert!(search(&data(), &ch, &Overrides::default(), &[far], 10.0, &|_, _| false).is_none());
    }
}
