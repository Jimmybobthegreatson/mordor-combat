//! Tiny vector/quaternion helpers so the format crate does not depend on a math library.
//! Quaternions are `[x, y, z, w]`.

pub type Vec3 = [f32; 3];
pub type Quat = [f32; 4];

pub const IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];

pub fn qmul(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

pub fn qconj(q: Quat) -> Quat {
    [-q[0], -q[1], -q[2], q[3]]
}

/// Rotate `v` by the unit quaternion `q`.
pub fn qrot(q: Quat, v: Vec3) -> Vec3 {
    let [x, y, z, w] = q;
    let t = [2.0 * (y * v[2] - z * v[1]), 2.0 * (z * v[0] - x * v[2]), 2.0 * (x * v[1] - y * v[0])];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}

pub fn qnormalize(q: Quat) -> Quat {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len > 0.0 { [q[0] / len, q[1] / len, q[2] / len, q[3] / len] } else { IDENTITY }
}

pub fn qdot(a: Quat, b: Quat) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// Normalised lerp along the shortest arc, as the engine's animated rotation codec does.
pub fn nlerp(a: Quat, b: Quat, t: f32) -> Quat {
    let w = if qdot(a, b) < 0.0 { -t } else { t };
    let s = 1.0 - t;
    qnormalize([a[0] * s + b[0] * w, a[1] * s + b[1] * w, a[2] * s + b[2] * w, a[3] * s + b[3] * w])
}

pub fn vadd(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn vsub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn vlerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
