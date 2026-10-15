//! Checks the ports of Unity's native math against the player's own code: the test loads
//! the oracle build's `UnityPlayer.dylib` and calls `FromToQuaternionSafe`,
//! `InvertMatrix4x4_Full`, `Matrix4x4f::SetTRS`, `EulerToQuaternion` and the icall stubs of
//! `Vector3.RotateTowards`, `Quaternion.LookRotation` and `Quaternion.Internal_ToEulerRad`
//! (pure functions, found through the icall table) on generated inputs, comparing every result
//! bit for bit.
//! Ignored by default: it needs `builds/arm64-oracle`, and it fails if the player there is
//! another build (each function's first bytes are checked).

use std::ffi::{CStr, CString};

use super::cs::{self, M4, V3};
use super::funnel::from_to_rotation;

/// Offsets in the arm64 player (`UnityPlayer.dylib`, 25,064,735 bytes) and each
/// function's first 16 bytes.
const ANCHOR: (&str, usize, &str) =
    ("CopyMatrix4x4_NEON", 0xeed160, "00a8df4c02a8404c20a89f4c22a8004c");
const FROM_TO: (usize, &str) = (0x5c4ea4, "ffc301d1fd7b06a9fd8301910004402d");
const INVERSE: (usize, &str) = (0x5c32e8, "ff8302d1fd7b09a9fd430291e88400b0");
const TRS: (usize, &str) = (0x5c4010, "4004402d0228201e2328211e4414412d");
const EULER: (usize, &str) = (0x5bc874, "ed33bb6deb2b016de923026df44f03a9");
/// `Vector3.RotateTowards_Injected`, the icall stub (its out parameter avoids the indirect
/// result register the implementation uses).
const ROTATE_TOWARDS: (usize, &str) = (0x81be34, "ffc300d1f44f01a9fd7b02a9fd830091");
/// `Quaternion.LookRotation_Injected(ref forward, ref upwards, out ret)`, the icall stub.
const LOOK_ROTATION: (usize, &str) = (0x81c2d0, "f44fbea9fd7b01a9fd430091f30302aa");
/// `Quaternion.Internal_ToEulerRad_Injected(ref rotation, out ret)`, the icall stub.
const TO_EULER: (usize, &str) = (0x81c074, "ff0301d1f44f02a9fd7b03a9fdc30091");
/// `BoundsScripting::Contains(const AABB&, const Vector3f&)`, which the icall
/// `Bounds.Contains_Injected` (at 0x7ce45c) calls.
const BOUNDS_CONTAINS: (usize, &str) = (0x4b4fa4, "200040bd010040bd020c40bd2338221e");

#[repr(C)]
struct Quat([f32; 4]);

struct Player {
    base: usize,
}

impl Player {
    fn load() -> Self {
        let path = wr_paths::repo_root()
            .join("builds/arm64-oracle/Wrath.app/Contents/Frameworks/UnityPlayer.dylib");
        let c = CString::new(path.to_str().expect("utf-8 path")).expect("no NUL");
        // SAFETY: loading the player runs its static initialisers, which only set up
        // process-local state; nothing else in the test process uses it.
        let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if h.is_null() {
            // SAFETY: `dlopen` failed, so `dlerror` returns a C string.
            let err = unsafe { CStr::from_ptr(libc::dlerror()) };
            panic!("dlopen {}: {err:?}", path.display());
        }
        let sym = CString::new(ANCHOR.0).expect("no NUL");
        // SAFETY: `h` is a live handle and `sym` a C string.
        let anchor = unsafe { libc::dlsym(h, sym.as_ptr()) } as usize;
        assert_ne!(anchor, 0, "{} not exported", ANCHOR.0);
        let p = Self { base: anchor - ANCHOR.1 };
        for (off, bytes) in [
            (ANCHOR.1, ANCHOR.2),
            FROM_TO,
            INVERSE,
            TRS,
            EULER,
            ROTATE_TOWARDS,
            LOOK_ROTATION,
            TO_EULER,
            BOUNDS_CONTAINS,
        ] {
            // SAFETY: inside the loaded image's text segment.
            let have = unsafe { std::slice::from_raw_parts((p.base + off) as *const u8, 16) };
            let have: String = have.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(have, bytes, "another player build: the function at {off:#x} differs");
        }
        p
    }

    fn rotation_between(&self, f: V3, t: V3) -> [f32; 4] {
        // SAFETY: `Quaternionf FromToQuaternionSafe(const Vector3f&, const Vector3f&)`.
        let fun: extern "C" fn(*const V3, *const V3) -> Quat =
            unsafe { std::mem::transmute(self.base + FROM_TO.0) };
        fun(&f, &t).0
    }

    /// `None` where the player reports a singular matrix.
    fn inverse(&self, m: &M4) -> Option<M4> {
        // SAFETY: `bool InvertMatrix4x4_Full(const float* in, float* out)`, column-major.
        let fun: extern "C" fn(*const [f32; 16], *mut [f32; 16]) -> bool =
            unsafe { std::mem::transmute(self.base + INVERSE.0) };
        let mut out = [0.0f32; 16];
        fun(&to_cols(m), &mut out).then(|| from_cols(&out))
    }

    fn trs(&self, t: V3, q: [f32; 4], s: V3) -> M4 {
        // SAFETY: `Matrix4x4f::SetTRS(this, const Vector3f&, const Quaternionf&, const Vector3f&)`.
        let fun: extern "C" fn(*mut [f32; 16], *const V3, *const [f32; 4], *const V3) =
            unsafe { std::mem::transmute(self.base + TRS.0) };
        let mut out = [0.0f32; 16];
        fun(&mut out, &t, &q, &s);
        from_cols(&out)
    }

    /// `Quaternion.Euler(deg)`: the managed side's `euler * Mathf.Deg2Rad`, then the
    /// native conversion in Unity's default order (4, Z-X-Y).
    fn euler(&self, deg: V3) -> [f32; 4] {
        // SAFETY: `Quaternionf EulerToQuaternion(const Vector3f&, RotationOrder)`.
        let fun: extern "C" fn(*const V3, u32) -> Quat =
            unsafe { std::mem::transmute(self.base + EULER.0) };
        let rad = deg.map(|x| x * 0.017_453_292_f32);
        fun(&rad, 4).0
    }
}

impl Player {
    /// `Vector3.RotateTowards_Injected(ref current, ref target, maxRadiansDelta,
    /// maxMagnitudeDelta, out ret)`.
    fn rotate_towards(&self, a: V3, b: V3, max_radians: f32, max_magnitude: f32) -> V3 {
        // SAFETY: the icall stub's signature: two vector pointers, two floats, an out pointer.
        let fun: extern "C" fn(*const V3, *const V3, f32, f32, *mut V3) =
            unsafe { std::mem::transmute(self.base + ROTATE_TOWARDS.0) };
        let mut out = [0.0f32; 3];
        fun(&a, &b, max_radians, max_magnitude, &mut out);
        out
    }

    fn look_rotation(&self, forward: V3, up: V3) -> [f32; 4] {
        // SAFETY: the icall stub's signature: two vector pointers, an out quaternion.
        let fun: extern "C" fn(*const V3, *const V3, *mut [f32; 4]) =
            unsafe { std::mem::transmute(self.base + LOOK_ROTATION.0) };
        let mut out = [0.0f32; 4];
        fun(&forward, &up, &mut out);
        out
    }

    fn to_euler_rad(&self, q: [f32; 4]) -> V3 {
        // SAFETY: the icall stub's signature: a quaternion pointer, an out vector.
        let fun: extern "C" fn(*const [f32; 4], *mut V3) =
            unsafe { std::mem::transmute(self.base + TO_EULER.0) };
        let mut out = [0.0f32; 3];
        fun(&q, &mut out);
        out
    }
}

/// Points stdout at `/dev/null` until dropped (the player's log lines go there).
struct Quiet(libc::c_int);

impl Quiet {
    fn stdout() -> Self {
        // SAFETY: plain descriptor calls; the saved descriptor is restored on drop.
        unsafe {
            libc::fflush(std::ptr::null_mut());
            let saved = libc::dup(1);
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
            libc::dup2(null, 1);
            libc::close(null);
            Self(saved)
        }
    }
}

impl Drop for Quiet {
    fn drop(&mut self) {
        // SAFETY: as above.
        unsafe {
            libc::fflush(std::ptr::null_mut());
            libc::dup2(self.0, 1);
            libc::close(self.0);
        }
    }
}

/// Does `ToEuler` take its pole branch for `q` (normalised as it normalises it)?
fn at_pole(q: [f32; 4]) -> bool {
    let m = (((q[0] * q[0] + q[1] * q[1]) + q[2] * q[2]) + q[3] * q[3]).sqrt();
    let [x, y, z, w] = if m < 1e-5 { [0.0, 0.0, 0.0, 1.0] } else { q.map(|c| c / m) };
    (y * z - x * w).abs() >= f32::from_bits(0x3eff_ffde)
}

impl Player {
    /// `Bounds.Contains(point)` on `Bounds { m_Center, m_Extents }`.
    fn bounds_contains(&self, center: V3, extents: V3, p: V3) -> bool {
        // SAFETY: `bool BoundsScripting::Contains(const AABB&, const Vector3f&)`; `AABB` is
        // the centre then the extents, six floats.
        let fun: extern "C" fn(*const [f32; 6], *const V3) -> bool =
            unsafe { std::mem::transmute(self.base + BOUNDS_CONTAINS.0) };
        let aabb = [center[0], center[1], center[2], extents[0], extents[1], extents[2]];
        fun(&aabb, &p)
    }
}

fn to_cols(m: &M4) -> [f32; 16] {
    std::array::from_fn(|i| m[i % 4][i / 4])
}

fn from_cols(c: &[f32; 16]) -> M4 {
    std::array::from_fn(|r| std::array::from_fn(|col| c[r + col * 4]))
}

fn bits<const N: usize>(a: [f32; N]) -> [u32; N] {
    a.map(f32::to_bits)
}

/// xorshift64*, for reproducible inputs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// A component: uniform, a small integer, a tiny value or a zero of either sign.
    fn comp(&mut self, scale: f32) -> f32 {
        match self.next() % 8 {
            0 => {
                if self.next().is_multiple_of(2) {
                    0.0
                } else {
                    -0.0
                }
            }
            1 => (self.next() % 9) as f32 - 4.0,
            2 => (self.unit() - 0.5) * 1e-4,
            _ => (self.unit() * 2.0 - 1.0) * scale,
        }
    }

    fn v3(&mut self, scale: f32) -> V3 {
        [self.comp(scale), self.comp(scale), self.comp(scale)]
    }
}

#[test]
#[ignore = "needs the oracle build's UnityPlayer.dylib"]
fn native_math_matches_the_player() {
    let p = Player::load();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let n = 1_000_000;

    // FromToRotation: general directions, near-parallel and near-antiparallel pairs (both
    // thresholds and the 180° case), and lengths around the 1e-5 cut.
    let mut checked = [0usize; 4];
    for i in 0..n {
        let f = rng.v3(100.0);
        let t = match i % 4 {
            0 => rng.v3(100.0),
            1 | 2 => {
                let sign = if i % 4 == 1 { 1.0 } else { -1.0 };
                let k = rng.unit() * 10.0 + 0.01;
                let spread = 1e-3 * rng.unit();
                let e = rng.v3(spread);
                [f[0] * k * sign + e[0], f[1] * k * sign + e[1], f[2] * k * sign + e[2]]
            }
            _ => rng.v3(2e-5),
        };
        let want = p.rotation_between(f, t);
        assert_eq!(
            bits(from_to_rotation(f, t)),
            bits(want),
            "FromToRotation({f:?}, {t:?}) = {want:?}"
        );
        checked[i % 4] += 1;
    }

    // Quaternion.Euler, Matrix4x4.TRS and the general inverse, on transforms built like
    // `GridGraph.CalculateTransform`'s and on sparse random matrices.
    let mut singular = 0;
    for i in 0..n {
        let deg = match i % 4 {
            0 | 1 => rng.v3(360.0),
            2 => [0.0, rng.comp(720.0), 0.0],
            // Subnormal radians: the order of `* Deg2Rad` and `* 0.5` shows.
            _ => [0, 1, 2].map(|_| f32::from_bits((rng.next() as u32) & 0x807f_ffff) * 57.0),
        };
        let q = p.euler(deg);
        assert_eq!(bits(cs::quat_euler(deg)), bits(q), "Euler({deg:?})");
        let (t, s) = (rng.v3(500.0), rng.v3(4.0));
        let m = p.trs(t, q, s);
        assert_eq!(cs::trs(t, q, s).map(bits), m.map(bits), "TRS({t:?}, {q:?}, {s:?})");
        let m = if i % 3 == 0 {
            let mut sparse = [[0.0f32; 4]; 4];
            for x in sparse.as_flattened_mut() {
                *x = match rng.next() % 6 {
                    0 => 0.0,
                    1 => -0.0,
                    _ => rng.comp(10.0),
                };
            }
            sparse
        } else {
            m
        };
        let want = p.inverse(&m);
        if want.is_none() {
            singular += 1;
        }
        assert_eq!(
            cs::mat_inverse(&m).map(|m| m.map(bits)),
            want.map(|m| m.map(bits)),
            "inverse({m:?})"
        );
    }
    // RotateTowards as TickMovement calls it (2D directions as Vector3s, radians per tick, 1)
    // and generally: unit and random lengths, near-parallel and antiparallel pairs, zero.
    for i in 0..n {
        let a = rng.v3(2.0);
        let b = match i % 4 {
            0 => rng.v3(2.0),
            1 => [-a[0] + rng.comp(1e-3), -a[1] + rng.comp(1e-3), -a[2] + rng.comp(1e-3)],
            2 => [a[0] + rng.comp(1e-3), a[1] + rng.comp(1e-3), a[2] + rng.comp(1e-3)],
            _ => [rng.comp(1.0), rng.comp(1.0), 0.0],
        };
        let (a, b) = if i % 4 == 3 { ([a[0], a[1], 0.0], b) } else { (a, b) };
        let max_radians = rng.comp(0.5);
        let max_magnitude = if i % 2 == 0 { 1.0 } else { rng.comp(3.0) };
        let want = p.rotate_towards(a, b, max_radians, max_magnitude);
        assert_eq!(
            bits(cs::rotate_towards(a, b, max_radians, max_magnitude)),
            bits(want),
            "RotateTowards({a:?}, {b:?}, {max_radians}, {max_magnitude}) = {want:?}"
        );
    }
    // LookRotation as UnitMoveController calls it (an XZ direction, up = +Y) and generally:
    // zero and tiny forwards around the 1e-5 cuts, forwards (anti)parallel to up or nearly so
    // (the fallback through `SetFromToRotation`), random and zero ups. The player logs each
    // zero forward to stdout, silenced meanwhile.
    let mut fallback = 0;
    let quiet = Quiet::stdout();
    for i in 0..n {
        let up = match i % 8 {
            0 => rng.v3(2.0),
            1 => [0.0, 0.0, 0.0],
            _ => [0.0, 1.0, 0.0],
        };
        let f = match i % 6 {
            0 => rng.v3(100.0),
            1 => [rng.comp(2.0), 0.0, rng.comp(2.0)],
            2 => rng.v3(2e-5),
            3 => {
                let k = rng.comp(10.0);
                let e = rng.unit() * 1e-3;
                [up[0] * k + rng.comp(e), up[1] * k + rng.comp(e), up[2] * k + rng.comp(e)]
            }
            4 => [rng.comp(1e-5), rng.comp(4.0), rng.comp(1e-5)],
            _ => [0.0, rng.comp(4.0), 0.0],
        };
        let want = p.look_rotation(f, up);
        let ours = cs::look_rotation(f, up);
        assert_eq!(bits(ours), bits(want), "LookRotation({f:?}, {up:?}) = {want:?}");
        if cs::look_rotation_matrix(f, up).is_none() && cs::native_magnitude(f) > 1e-5 {
            fallback += 1;
        }
    }
    drop(quiet);
    // Internal_ToEulerRad: rotations LookRotation makes, unit quaternions from Euler angles,
    // near the poles (x = ±90°, where `y` comes from the product terms and `z` is 0),
    // unnormalised and tiny quaternions.
    let mut poles = 0;
    for i in 0..n {
        let q = match i % 6 {
            0 => cs::look_rotation([rng.comp(2.0), 0.0, rng.comp(2.0)], [0.0, 1.0, 0.0]),
            1 => cs::quat_euler(rng.v3(720.0)),
            2 | 3 => {
                let x = if rng.next().is_multiple_of(2) { 90.0 } else { -90.0 };
                let e = if i % 6 == 2 { rng.comp(1e-2) } else { rng.comp(2.0) };
                let q = cs::quat_euler([x + e, rng.comp(360.0), rng.comp(360.0)]);
                let k = rng.unit() * 4.0 + 1e-3;
                q.map(|c| c * k)
            }
            4 => [rng.comp(3.0), rng.comp(3.0), rng.comp(3.0), rng.comp(3.0)],
            _ => [rng.comp(1e-5), rng.comp(1e-5), rng.comp(1e-5), rng.comp(1e-5)],
        };
        let want = p.to_euler_rad(q);
        assert_eq!(bits(cs::to_euler_rad(q)), bits(want), "ToEulerRad({q:?}) = {want:?}");
        if at_pole(q) {
            poles += 1;
        }
    }
    // The thresholds themselves and non-finite inputs: forward lengths a few ulps around 1e-5
    // (along up and across it), NaN and infinite components; x angles swept across the pole
    // threshold (89.885°) in 1e-5° steps.
    let quiet = Quiet::stdout();
    let cut = 1e-5_f32.to_bits();
    let odd = [f32::NAN, -f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
    let mut edges = 0;
    for b in cut - 4..=cut + 4 {
        let m = f32::from_bits(b);
        for f in [[m, 0.0, 0.0], [0.0, m, 0.0], [0.0, -m, 0.0], [0.0, 0.0, m], [m, m, 0.0]] {
            for up in [[0.0, 1.0, 0.0], [m, 1.0, 0.0], [1.0, 0.0, 0.0]] {
                let want = p.look_rotation(f, up);
                assert_eq!(
                    bits(cs::look_rotation(f, up)),
                    bits(want),
                    "LookRotation({f:?}, {up:?})"
                );
                edges += 1;
            }
        }
    }
    for x in odd {
        for f in [[x, 0.0, 1.0], [0.0, x, 0.0], [1.0, 0.0, x]] {
            for up in [[0.0, 1.0, 0.0], [x, 1.0, 0.0]] {
                let want = p.look_rotation(f, up);
                assert_eq!(
                    bits(cs::look_rotation(f, up)),
                    bits(want),
                    "LookRotation({f:?}, {up:?})"
                );
                edges += 1;
            }
        }
        for q in [[x, 0.0, 0.0, 1.0], [0.0, x, 0.0, 1.0], [0.5, 0.5, x, 0.5], [0.0, 0.0, 0.0, x]] {
            let want = p.to_euler_rad(q);
            assert_eq!(bits(cs::to_euler_rad(q)), bits(want), "ToEulerRad({q:?}) = {want:?}");
            edges += 1;
        }
    }
    drop(quiet);
    for k in 0..20_000 {
        let x = 89.88 + k as f32 * 1e-5;
        let x = if k % 2 == 0 { x } else { -x };
        let q = cs::quat_euler([x, rng.comp(360.0), rng.comp(360.0)]);
        let want = p.to_euler_rad(q);
        assert_eq!(bits(cs::to_euler_rad(q)), bits(want), "ToEulerRad({q:?}) = {want:?}");
        if at_pole(q) {
            poles += 1;
        }
        edges += 1;
    }
    eprintln!("{edges} threshold and non-finite cases: bit-identical");
    // Bounds.Contains: points on and around the faces (grid cells against the blockers'
    // boxes), zero extents (the clearance box's height), NaN.
    let mut inside = 0;
    for i in 0..n {
        let c = rng.v3(100.0);
        let e = match i % 4 {
            0 => rng.v3(2.0),
            1 => [rng.comp(1.0), 0.0, rng.comp(1.0)],
            _ => rng.v3(1.0).map(f32::abs),
        };
        let mut q = c;
        for (k, x) in q.iter_mut().enumerate() {
            let face = if rng.next().is_multiple_of(2) { c[k] + e[k] } else { c[k] - e[k] };
            *x = match rng.next() % 5 {
                0 => face,
                1 => f32::from_bits(face.to_bits().wrapping_add(1)),
                2 => f32::from_bits(face.to_bits().wrapping_sub(1)),
                3 => c[k] + rng.comp(3.0),
                _ => {
                    if i % 97 == 0 {
                        f32::NAN
                    } else {
                        c[k]
                    }
                }
            };
        }
        let want = p.bounds_contains(c, e, q);
        let b = crate::update::Bounds { center: c, extents: e };
        assert_eq!(b.contains(q), want, "Bounds({c:?}, {e:?}).Contains({q:?})");
        inside += usize::from(want);
    }
    eprintln!(
        "from_to {checked:?}, {n} matrices ({singular} singular), {n} rotations, {n} look \
         rotations ({fallback} by the fallback), {n} euler conversions ({poles} at a pole), {n} \
         bounds ({inside} inside): all bit-identical"
    );
}
