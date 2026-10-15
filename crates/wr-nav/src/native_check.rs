//! Checks the ports of Unity's native math against the player's own code: the test loads
//! the oracle build's `UnityPlayer.dylib` and calls `FromToQuaternionSafe`,
//! `InvertMatrix4x4_Full`, `Matrix4x4f::SetTRS` and `EulerToQuaternion` (pure functions,
//! found through the icall table) on generated inputs, comparing every result bit for bit.
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
        for (off, bytes) in [(ANCHOR.1, ANCHOR.2), FROM_TO, INVERSE, TRS, EULER, ROTATE_TOWARDS] {
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
    eprintln!(
        "from_to {checked:?}, {n} matrices ({singular} singular), {n} rotations: all bit-identical"
    );
}
