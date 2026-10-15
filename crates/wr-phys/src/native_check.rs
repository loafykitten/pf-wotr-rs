//! Checks the height field raycast against the player's own PhysX: the test loads the oracle
//! build's `UnityPlayer.dylib` and calls its `raycast_heightField` (the registered entry of
//! `Gu::gRaycastMap`, found through the table that `getRaycastFuncTable` returns and the
//! unregistered stub's error string) on generated height fields and rays, every result
//! compared bit for bit with [`HeightField::raycast`]. Ignored by default: it needs
//! `builds/arm64-oracle`, and it fails if the player there is another build (each function's
//! first bytes are checked).
//!
//! The height field is built in memory with `Gu::HeightField`'s layout as the function reads it
//! (`mData` at 0x20: the `CenterExtents` bounds, `rows` 0x38, `columns` 0x3c, `samples` 0x50).

use std::ffi::{CStr, CString};

use crate::hf::{HeightField, Sample};
use crate::px::{Quat, Transform, Vec3};

/// The exported anchor (`CopyMatrix4x4_NEON`, as `wr-nav`'s check uses) and its first bytes.
const ANCHOR: (&str, usize, &str) =
    ("CopyMatrix4x4_NEON", 0xeed160, "00a8df4c02a8404c20a89f4c22a8004c");
/// `Gu::raycast_heightField` (`GuRaycastTests.cpp`).
const RAYCAST_HF: (usize, &str) = (0x1326074, "ef3bb66ded33016deb2b026de923036d");
/// `Gu::raycast_box`, `gRaycastMap[eBOX]`.
const RAYCAST_BOX: (usize, &str) = (0x1324e48, "ff4302d1e923046df85f05a9f65706a9");

/// `PxBoxGeometry`.
#[repr(C)]
struct BoxGeometry {
    kind: i32,
    half_extents: [f32; 3],
}

/// `PxHeightFieldGeometry`.
#[repr(C)]
struct HfGeometry {
    kind: i32,
    height_field: *const u8,
    height_scale: f32,
    row_scale: f32,
    column_scale: f32,
    flags: u8,
    _pad: [u8; 3],
}

/// `PxRaycastHit` (64 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct PxRaycastHit {
    actor: usize,
    shape: usize,
    face_index: u32,
    flags: u16,
    _pad: u16,
    position: [f32; 3],
    normal: [f32; 3],
    distance: f32,
    u: f32,
    v: f32,
    _pad2: u32,
}

type RaycastFn<G> = extern "C" fn(
    *const G,
    *const [f32; 7],
    *const [f32; 3],
    *const [f32; 3],
    f32,
    *const u16,
    u32,
    *mut PxRaycastHit,
) -> u32;

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
        for (off, bytes) in [(ANCHOR.1, ANCHOR.2), RAYCAST_HF, RAYCAST_BOX] {
            // SAFETY: inside the loaded image's text segment.
            let have = unsafe { std::slice::from_raw_parts((p.base + off) as *const u8, 16) };
            let have: String = have.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(have, bytes, "another player build: the function at {off:#x} differs");
        }
        p
    }

    /// The player's `raycast_heightField` on `field` (one hit, `maxHits` 1).
    fn raycast(
        &self,
        field: &HeightField,
        pose: Transform,
        origin: Vec3,
        dir: Vec3,
        max: f32,
        both_sides: bool,
    ) -> Option<(f32, [f32; 3], u32)> {
        // `Gu::HeightField` as the function reads it.
        let samples: Vec<[u8; 4]> = field
            .samples
            .iter()
            .map(|s| {
                let h = s.height.to_le_bytes();
                [h[0], h[1], s.material0, s.material1]
            })
            .collect();
        let mut obj = vec![0u8; 0x100];
        let (center, extents) = field.data_aabb();
        let put = |o: &mut [u8], at: usize, v: f32| o[at..at + 4].copy_from_slice(&v.to_le_bytes());
        for (k, v) in
            [center.x, center.y, center.z, extents.x, extents.y, extents.z].into_iter().enumerate()
        {
            put(&mut obj, 0x20 + 4 * k, v);
        }
        obj[0x38..0x3c].copy_from_slice(&field.rows.to_le_bytes());
        obj[0x3c..0x40].copy_from_slice(&field.columns.to_le_bytes());
        put(&mut obj, 0x40, field.rows as f32 - 2.0);
        put(&mut obj, 0x44, field.columns as f32 - 2.0);
        put(&mut obj, 0x48, field.columns as f32);
        obj[0x50..0x58].copy_from_slice(&(samples.as_ptr() as usize).to_le_bytes());
        let geom = HfGeometry {
            kind: 6, // PxGeometryType::eHEIGHTFIELD
            height_field: obj.as_ptr(),
            height_scale: field.height_scale,
            row_scale: field.row_scale,
            column_scale: field.column_scale,
            flags: if field.double_sided { 2 } else { 0 },
            _pad: [0; 3],
        };
        let pose7 = [pose.q.x, pose.q.y, pose.q.z, pose.q.w, pose.p.x, pose.p.y, pose.p.z];
        // Unity's flags: ePOSITION | eNORMAL | eUV, with eMESH_BOTH_SIDES on request.
        let flags: u16 = 0xb | if both_sides { 0x80 } else { 0 };
        // SAFETY: the zeroed hit has no invalid bit patterns.
        let mut hit: PxRaycastHit = unsafe { std::mem::zeroed() };
        // SAFETY: `PxU32 raycast_heightField(const PxGeometry&, const PxTransform&, const PxVec3&,
        // const PxVec3&, PxReal, PxHitFlags (indirect), PxU32, PxRaycastHit*)`; every pointer
        // is live for the call and the height field object is laid out as the function reads it.
        let fun: RaycastFn<HfGeometry> = unsafe { std::mem::transmute(self.base + RAYCAST_HF.0) };
        let n = fun(&geom, &pose7, &origin.to_array(), &dir.to_array(), max, &flags, 1, &mut hit);
        drop(samples);
        drop(obj);
        (n == 1).then_some((hit.distance, hit.position, hit.face_index))
    }
}

impl Player {
    /// The player's `raycast_box` (`ePOSITION | eNORMAL | eUV`, one hit).
    fn raycast_box(
        &self,
        half: Vec3,
        pose: Transform,
        origin: Vec3,
        dir: Vec3,
        max: f32,
    ) -> Option<(f32, [f32; 3])> {
        let geom = BoxGeometry { kind: 3, half_extents: half.to_array() };
        let pose7 = [pose.q.x, pose.q.y, pose.q.z, pose.q.w, pose.p.x, pose.p.y, pose.p.z];
        let flags: u16 = 0xb;
        // SAFETY: the zeroed hit has no invalid bit patterns.
        let mut hit: PxRaycastHit = unsafe { std::mem::zeroed() };
        // SAFETY: `raycast_box` has the raycast functions' signature; every pointer is live.
        let fun: RaycastFn<BoxGeometry> = unsafe { std::mem::transmute(self.base + RAYCAST_BOX.0) };
        let n = fun(&geom, &pose7, &origin.to_array(), &dir.to_array(), max, &flags, 1, &mut hit);
        (n == 1).then_some((hit.distance, hit.position))
    }
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

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn random_field(r: &mut Rng) -> HeightField {
    let rows = 2 + r.below(10) as u32;
    let columns = 2 + r.below(10) as u32;
    let spread = [10, 300, 4000, 32000][r.below(4) as usize];
    let base = r.below(2000) as i32 - 1000;
    let samples = (0..rows * columns)
        .map(|_| {
            let h = (base + r.below(2 * spread as u64) as i32 - spread).clamp(-32768, 32767) as i16;
            let tess = if r.below(4) == 0 { 0 } else { 0x80 };
            let hole0 = if r.below(10) == 0 { 127 } else { r.below(3) as u8 };
            let hole1 = if r.below(10) == 0 { 127 } else { r.below(3) as u8 };
            // materialIndex1's high bit is PhysX's own mark: set at random, never read here.
            Sample {
                height: h,
                material0: tess | hole0,
                material1: hole1 | (r.below(2) as u8) << 7,
            }
        })
        .collect();
    let sign = |r: &mut Rng| if r.below(8) == 0 { -1.0 } else { 1.0 };
    HeightField {
        rows,
        columns,
        samples,
        height_scale: r.range(0.0005, 0.05),
        row_scale: sign(r) * r.range(0.05, 3.0),
        column_scale: sign(r) * r.range(0.05, 3.0),
        double_sided: r.below(4) == 0,
    }
}

fn random_pose(r: &mut Rng) -> Transform {
    if r.below(3) == 0 {
        return Transform {
            q: Quat::IDENTITY,
            p: Vec3::new(r.range(-50.0, 50.0), r.range(-50.0, 50.0), r.range(-50.0, 50.0)),
        };
    }
    let (x, y, z, w) =
        (r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0));
    let n = (x * x + y * y + z * z + w * w).sqrt();
    Transform {
        q: Quat { x: x / n, y: y / n, z: z / n, w: w / n },
        p: Vec3::new(r.range(-50.0, 50.0), r.range(-50.0, 50.0), r.range(-50.0, 50.0)),
    }
}

#[test]
#[ignore = "needs builds/arm64-oracle (the player's UnityPlayer.dylib)"]
fn raycast_height_field_matches_the_player() {
    let p = Player::load();
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut n, mut hits, mut down, mut differ) = (0usize, 0usize, 0usize, 0usize);
    let mut bad = Vec::new();
    for _ in 0..4000 {
        let field = random_field(&mut r);
        let pose = random_pose(&mut r);
        // Rays from around the field (local), mapped to world or not.
        let (lo, hi) = field
            .samples
            .iter()
            .fold((i16::MAX, i16::MIN), |(a, b), s| (a.min(s.height), b.max(s.height)));
        let (lo, hi) = (f32::from(lo) * field.height_scale, f32::from(hi) * field.height_scale);
        for _ in 0..250 {
            // Often on a grid line or vertex (the traversal's and the triangle tests' edges).
            let grid = |r: &mut Rng, cells: u32, scale: f32| {
                let k = r.below(u64::from(cells) + 1) as f32;
                match r.below(4) {
                    0 => k * scale,
                    1 => f32::from_bits(
                        (k * scale).to_bits().wrapping_add(r.below(3) as u32).wrapping_sub(1),
                    ),
                    _ => r.range(-0.2, 1.2) * (cells as f32 * scale),
                }
            };
            let lx = grid(&mut r, field.rows - 1, field.row_scale);
            let lz = grid(&mut r, field.columns - 1, field.column_scale);
            let ly = match r.below(3) {
                0 => r.range(lo - 1.0, hi + 3.0),
                1 => hi + r.range(0.0, 0.01),
                _ => r.range(-100.0, 100.0) + (lo + hi) * 0.5,
            };
            let local = Vec3::new(lx, ly, lz);
            let origin = if r.below(3) == 0 { local } else { pose.transform(local) };
            let pose = if origin == local { Transform::IDENTITY } else { pose };
            let dir = match r.below(6) {
                0 | 1 => Vec3::new(0.0, -1.0, 0.0),
                2 => pose.rotate(Vec3::new(0.0, -1.0, 0.0)),
                3 => {
                    // Grazing.
                    let d = Vec3::new(r.range(-1.0, 1.0), r.range(-0.02, 0.02), r.range(-1.0, 1.0));
                    pose.rotate(d.scale(1.0 / d.magnitude().max(1e-3)))
                }
                _ => {
                    let d = Vec3::new(r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0));
                    let m = d.magnitude();
                    if m < 1e-3 { Vec3::new(0.0, -1.0, 0.0) } else { d.scale(1.0 / m) }
                }
            };
            let max = match r.below(4) {
                0 => 100.0,
                1 => r.range(0.0, 10.0),
                2 => r.range(0.0, 1000.0),
                _ => 1.0e7,
            };
            let both = r.below(5) == 0;
            let want = p.raycast(&field, pose, origin, dir, max, both);
            let got = field
                .raycast(pose, origin, dir, max, both)
                .map(|h| (h.distance, h.position.to_array(), h.face_index));
            let key = |x: Option<(f32, [f32; 3], u32)>| {
                x.map(|(d, pt, f)| (d.to_bits(), pt.map(f32::to_bits), f))
            };
            n += 1;
            hits += usize::from(want.is_some());
            down += usize::from(dir == Vec3::new(0.0, -1.0, 0.0) && want.is_some());
            if key(want) != key(got) {
                differ += 1;
            }
            if key(want) != key(got) && bad.len() < 20 {
                bad.push(format!(
                    "{field:?} pose {pose:?} from {origin:?} dir {dir:?} max {max} both {both}: player {want:?}, ours {got:?}"
                ));
            }
        }
    }
    eprintln!("{n} rays ({hits} hits, {down} straight down), {differ} differ");
    for b in &bad {
        eprintln!("  {}", &b[b.len().saturating_sub(400)..]);
    }
    assert!(bad.is_empty());
}

#[test]
#[ignore = "needs builds/arm64-oracle (the player's UnityPlayer.dylib)"]
fn raycast_box_matches_the_player() {
    let p = Player::load();
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    let (mut n, mut hits, mut differ, mut missed) = (0usize, 0usize, 0usize, 0usize);
    let mut bad = Vec::new();
    for _ in 0..1_000_000 {
        let size = if r.below(2) == 0 { 5.0 } else { 50.0 };
        let half = Vec3::new(r.range(0.01, size), r.range(0.01, size), r.range(0.01, size));
        let mut pose = random_pose(&mut r);
        if r.below(2) == 0 {
            pose.p = pose.p.scale(20.0);
        }
        // From around the box, often straight down onto its top, sometimes from inside.
        let local = Vec3::new(
            r.range(-1.3, 1.3) * half.x,
            r.range(-1.3, 3.0) * half.y,
            r.range(-1.3, 1.3) * half.z,
        );
        let origin = pose.transform(local);
        let dir = match r.below(4) {
            0 => Vec3::new(0.0, -1.0, 0.0),
            1 => pose.rotate(Vec3::new(0.0, -1.0, 0.0)),
            2 => {
                // Grazing a face.
                let d = Vec3::new(r.range(-1.0, 1.0), r.range(-0.003, 0.003), r.range(-1.0, 1.0));
                pose.rotate(d.scale(1.0 / d.magnitude().max(1e-3)))
            }
            _ => {
                let d = Vec3::new(r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0));
                let m = d.magnitude();
                if m < 1e-3 { Vec3::new(0.0, -1.0, 0.0) } else { d.scale(1.0 / m) }
            }
        };
        let mut max = if r.below(2) == 0 { 100.0 } else { r.range(0.0, 10.0) };
        // Often ending exactly at the hit (the conservative test's end of the ray).
        if r.below(2) == 0
            && let Some((d, _)) = p.raycast_box(half, pose, origin, dir, max)
        {
            max = d;
        }
        let want = p.raycast_box(half, pose, origin, dir, max);
        let got = crate::boxes::raycast_box(half, pose, origin, dir, max)
            .map(|h| (h.distance, h.position.to_array()));
        let key = |x: Option<(f32, [f32; 3])>| x.map(|(d, pt)| (d.to_bits(), pt.map(f32::to_bits)));
        // The conservative test (the queries' fail-closed path) must count every hit the player
        // reports, for the box under the same pose as a matrix.
        if want.is_some() {
            let q = pose.q;
            let (x, y, z, w) = (f64::from(q.x), f64::from(q.y), f64::from(q.z), f64::from(q.w));
            let m = crate::conservative::M4([
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y + z * w),
                2.0 * (x * z - y * w),
                0.0,
                2.0 * (x * y - z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z + x * w),
                0.0,
                2.0 * (x * z + y * w),
                2.0 * (y * z - x * w),
                1.0 - 2.0 * (x * x + y * y),
                0.0,
                f64::from(pose.p.x),
                f64::from(pose.p.y),
                f64::from(pose.p.z),
                1.0,
            ]);
            let o = origin.to_array().map(f64::from);
            let d = dir.to_array().map(f64::from);
            let h = half.to_array().map(f64::from);
            if !crate::conservative::ray_hits_box(o, d, f64::from(max), &m, [0.0; 3], h) {
                missed += 1;
                if bad.len() < 20 {
                    bad.push(format!("conservative miss: half {half:?} pose {pose:?} from {origin:?} dir {dir:?} max {max}: player {want:?}"));
                }
            }
        }
        n += 1;
        hits += usize::from(want.is_some());
        if key(want) != key(got) {
            differ += 1;
            if bad.len() < 20 {
                bad.push(format!("half {half:?} pose {pose:?} from {origin:?} dir {dir:?} max {max}: player {want:?}, ours {got:?}"));
            }
        }
    }
    eprintln!("{n} rays ({hits} hits), {differ} differ, {missed} missed by the conservative test");
    for b in &bad {
        eprintln!("  {b}");
    }
    assert_eq!((differ, missed), (0, 0));
}
