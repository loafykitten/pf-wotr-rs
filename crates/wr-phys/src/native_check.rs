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

use crate::bv4::{Bv4Mesh, MeshScale};
use crate::hf::{HeightField, Sample};
use crate::px::{Quat, Transform, Vec3};

/// The exported anchor (`CopyMatrix4x4_NEON`, as `wr-nav`'s check uses) and its first bytes.
const ANCHOR: (&str, usize, &str) =
    ("CopyMatrix4x4_NEON", 0xeed160, "00a8df4c02a8404c20a89f4c22a8004c");
/// `Gu::raycast_heightField` (`GuRaycastTests.cpp`).
const RAYCAST_HF: (usize, &str) = (0x1326074, "ef3bb66ded33016deb2b026de923036d");
/// `Gu::raycast_box`, `gRaycastMap[eBOX]`.
const RAYCAST_BOX: (usize, &str) = (0x1324e48, "ff4302d1e923046df85f05a9f65706a9");
/// `Gu::raycast_triangleMesh`, `gRaycastMap[eTRIANGLEMESH]`: dispatches on the mesh's concrete
/// type (`Midphase::raycastTriangleMesh`) to `raycast_triangleMesh_BV4` (`0x13e68bc`) for a
/// BVH34 mesh.
const RAYCAST_MESH: (usize, &str) = (0x1326018, "ff8300d1fd7b01a9fd430091e70306aa");
const RAYCAST_MESH_BV4: (usize, &str) = (0x13e68bc, "ff8305d1eb2b0e6de9230f6dfc6f10a9");
/// `BV4_RaycastSingle`.
const BV4_RAYCAST_SINGLE: (usize, &str) = (0x13e2420, "fc6fbaa9fa6701a9f85f02a9f65703a9");
/// Unity's `Transform::TransformPoint` (the icall `Transform::TransformPoint_Injected` at
/// `0x82efe8` calls it): the transform's `TransformHierarchy` (at 0x38) and index (0x40); the
/// hierarchy's job fence (0x0), local transforms (0x18: 48 bytes each, `t`, `q`, `s` as
/// four-float vectors) and parent indices (0x20); the result through `x8`.
const TRANSFORM_POINT: (usize, &str) = (0x6744c0, "f657bda9f44f01a9fd7b02a9fd830091");
/// `BoxCollider`'s half extents from a scale and a second factor (`(1, 1, 1)` from
/// `BoxCollider::Create` and `set_size`): `m_Size` at 0x7c; with no shape (0x48) and no
/// list node (0x88) it only computes; the result through `x8`.
const BOX_EXTENTS: (usize, &str) = (0x6b5280, "e923bc6df65701a9f44f02a9fd7b03a9");

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
        for (off, bytes) in [
            (ANCHOR.1, ANCHOR.2),
            RAYCAST_HF,
            RAYCAST_BOX,
            RAYCAST_MESH,
            RAYCAST_MESH_BV4,
            BV4_RAYCAST_SINGLE,
            TRANSFORM_POINT,
            BOX_EXTENTS,
        ] {
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

/// Returned through `x8` (a composite over 16 bytes): the callee writes only the first 12
/// bytes, so it is received as `MaybeUninit` and only those are read.
#[repr(C)]
struct Indirect([f32; 6]);

/// The three floats the callee wrote.
fn written(out: &std::mem::MaybeUninit<Indirect>) -> [f32; 3] {
    let p = out.as_ptr().cast::<f32>();
    // SAFETY: the callee initialised the first three floats of the return storage.
    unsafe { [p.read(), p.add(1).read(), p.add(2).read()] }
}

impl Player {
    /// The player's `Transform::TransformPoint(p)` for a hierarchy whose local transforms run
    /// from the root down (`chain`, the transform's own last).
    fn transform_point(&self, chain: &[crate::boxes::LocalTrs], p: [f32; 3]) -> [f32; 3] {
        // `TransformX` per level: t, q, s as float4 (the fourth lanes zero).
        let locals: Vec<[[f32; 4]; 3]> = chain
            .iter()
            .map(|l| {
                let [x, y, z] = l.position;
                let [sx, sy, sz] = l.scale;
                [[x, y, z, 0.0], l.rotation, [sx, sy, sz, 0.0]]
            })
            .collect();
        let parents: Vec<i32> = (0..chain.len() as i32).map(|i| i - 1).collect();
        let mut hierarchy = [0usize; 8];
        hierarchy[3] = locals.as_ptr() as usize;
        hierarchy[4] = parents.as_ptr() as usize;
        let mut transform = [0usize; 9];
        transform[7] = hierarchy.as_ptr() as usize;
        transform[8] = chain.len() - 1;
        // SAFETY: the function reads the transform's hierarchy and index, the hierarchy's fence
        // (zero: no job to wait for), locals and parents, the point; it writes 12 bytes at x8.
        let fun: extern "C" fn(*const usize, *const [f32; 3]) -> std::mem::MaybeUninit<Indirect> =
            unsafe { std::mem::transmute(self.base + TRANSFORM_POINT.0) };
        let out = fun(transform.as_ptr(), &p);
        drop((locals, parents));
        written(&out)
    }

    /// The player's box half extents for `size` under `lossy` (the second factor one).
    fn box_extents(&self, size: [f32; 3], lossy: [f32; 3]) -> [f32; 3] {
        let mut collider = [0u8; 0x100];
        for (k, v) in size.iter().enumerate() {
            collider[0x7c + 4 * k..0x80 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
        let one = [1.0f32; 3];
        // SAFETY: with no shape and no list node the function only reads `m_Size` and its two
        // vectors, and writes 12 bytes at x8.
        let fun: extern "C" fn(
            *const u8,
            *const [f32; 3],
            *const [f32; 3],
        ) -> std::mem::MaybeUninit<Indirect> =
            unsafe { std::mem::transmute(self.base + BOX_EXTENTS.0) };
        let out = fun(collider.as_ptr(), &lossy, &one);
        written(&out)
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

// --- Triangle meshes ---------------------------------------------------------------------

/// `PxTriangleMeshGeometry`.
#[repr(C)]
struct MeshGeometry {
    kind: i32,
    scale: [f32; 3],
    rotation: [f32; 4],
    mesh_flags: u8,
    _pad: [u8; 7],
    mesh: *const u8,
}

/// A `Gu::BV4TriangleMesh` laid out as `raycast_triangleMesh_BV4` and `BV4_RaycastSingle` read
/// it: the concrete type at 0x8, `mGeomEpsilon` at 0x58, the `BV4Tree` at 0xd0 (its
/// `SourceMesh` pointer, `mLocalBounds`, `mNodes` at +0x20, `mInitData` +0x28, the coefficients
/// +0x2c and +0x38), and the `SourceMesh` (vertices at 0x8, triangle count 0x18, 32-bit
/// triangles 0x20, 16-bit ones 0x28).
struct NativeMesh {
    obj: Vec<u128>,
    _source: Vec<u128>,
    _verts: Vec<f32>,
    _tris: Vec<u32>,
    _nodes: Vec<u128>,
}

impl NativeMesh {
    fn new(m: &Bv4Mesh) -> Self {
        // One spare float: the leaf copies each vertex with a 16-byte load.
        let mut verts: Vec<f32> = m.vertices.iter().flatten().copied().collect();
        verts.push(0.0);
        let tris: Vec<u32> = m.triangles.iter().flatten().copied().collect();
        let nodes: Vec<u128> = m
            .tree
            .nodes
            .iter()
            .map(|w| {
                u128::from(w[0])
                    | u128::from(w[1]) << 32
                    | u128::from(w[2]) << 64
                    | u128::from(w[3]) << 96
            })
            .collect();
        let mut source = vec![0u128; 4];
        let mut obj = vec![0u128; 0x200 / 16];
        let put = |b: &mut [u8], at: usize, v: &[u8]| b[at..at + v.len()].copy_from_slice(v);
        {
            // SAFETY: plain bytes of the owned, zeroed buffer.
            let s = unsafe {
                std::slice::from_raw_parts_mut(source.as_mut_ptr().cast::<u8>(), source.len() * 16)
            };
            put(s, 0, &(m.vertices.len() as u32).to_le_bytes());
            put(s, 0x8, &(verts.as_ptr() as usize).to_le_bytes());
            put(s, 0x18, &(m.triangles.len() as u32).to_le_bytes());
            put(s, 0x20, &(tris.as_ptr() as usize).to_le_bytes());
        }
        {
            // SAFETY: as above.
            let o = unsafe {
                std::slice::from_raw_parts_mut(obj.as_mut_ptr().cast::<u8>(), obj.len() * 16)
            };
            put(o, 0x8, &4u16.to_le_bytes()); // PxConcreteType::eTRIANGLE_MESH_BVH34
            put(o, 0x58, &m.geom_epsilon.to_le_bytes());
            let t = &m.tree;
            put(o, 0xd0, &(source.as_ptr() as usize).to_le_bytes());
            for (k, v) in t.center.iter().chain([&t.extents_magnitude]).enumerate() {
                put(o, 0xd8 + 4 * k, &v.to_le_bytes());
            }
            put(o, 0xe8, &(t.nodes.len() as u32).to_le_bytes());
            let np = if nodes.is_empty() { 0 } else { nodes.as_ptr() as usize };
            put(o, 0xf0, &np.to_le_bytes());
            put(o, 0xf8, &t.init_data.to_le_bytes());
            for (k, v) in t.min_coeff.iter().chain(&t.max_coeff).enumerate() {
                put(o, 0xfc + 4 * k, &v.to_le_bytes());
            }
            o[0x115] = 1; // mQuantized
        }
        Self { obj, _source: source, _verts: verts, _tris: tris, _nodes: nodes }
    }
}

/// A mesh hit: distance, position, face index, u, v.
type MeshOut = Option<(f32, [f32; 3], u32, f32, f32)>;

/// The same compared bit for bit.
fn mesh_key(h: MeshOut) -> Option<(u32, [u32; 3], u32, u32, u32)> {
    h.map(|(d, p, f, u, v)| (d.to_bits(), p.map(f32::to_bits), f, u.to_bits(), v.to_bits()))
}

/// One generated query on a mesh.
struct MeshCase {
    scale: MeshScale,
    double_sided: bool,
    pose: Transform,
    origin: Vec3,
    dir: Vec3,
    max: f32,
    flags: u16,
    /// End the ray exactly at the player's hit, when there is one.
    clip_to_hit: bool,
}

impl Player {
    /// The player's `raycast_triangleMesh` (one hit).
    fn raycast_mesh(&self, mesh: &NativeMesh, c: &MeshCase, max: f32) -> MeshOut {
        let q = c.scale.rotation;
        let geom = MeshGeometry {
            kind: 5, // PxGeometryType::eTRIANGLEMESH
            scale: c.scale.scale.to_array(),
            rotation: [q.x, q.y, q.z, q.w],
            mesh_flags: if c.double_sided { 2 } else { 0 },
            _pad: [0; 7],
            mesh: mesh.obj.as_ptr().cast(),
        };
        let pose = c.pose;
        let pose7 = [pose.q.x, pose.q.y, pose.q.z, pose.q.w, pose.p.x, pose.p.y, pose.p.z];
        // SAFETY: the zeroed hit has no invalid bit patterns.
        let mut hit: PxRaycastHit = unsafe { std::mem::zeroed() };
        // SAFETY: `raycast_triangleMesh` has the raycast functions' signature; every pointer is
        // live for the call and the mesh is laid out as the player reads it.
        let fun: RaycastFn<MeshGeometry> =
            unsafe { std::mem::transmute(self.base + RAYCAST_MESH.0) };
        let (o, d) = (c.origin.to_array(), c.dir.to_array());
        let n = fun(&geom, &pose7, &o, &d, max, &c.flags, 1, &mut hit);
        (n == 1).then_some((hit.distance, hit.position, hit.face_index, hit.u, hit.v))
    }
}

fn unit(r: &mut Rng) -> Vec3 {
    loop {
        let x = f64::from(r.range(-1.0, 1.0));
        let y = f64::from(r.range(-1.0, 1.0));
        let z = f64::from(r.range(-1.0, 1.0));
        let m = (x * x + y * y + z * z).sqrt();
        if m > 1e-3 && m <= 1.0 {
            return Vec3::new((x / m) as f32, (y / m) as f32, (z / m) as f32);
        }
    }
}

fn random_quat(r: &mut Rng) -> Quat {
    let (x, y, z, w) =
        (r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-1.0, 1.0));
    let n = (x * x + y * y + z * z + w * w).sqrt().max(1e-3);
    Quat { x: x / n, y: y / n, z: z / n, w: w / n }
}

/// A query on `mesh`, whose collider sits at `shape_pose` with `lossy`: the collider's pose
/// or another, an identity, lossy, near-one or arbitrary scale, a ray at a point on a
/// triangle (inside, on an edge or vertex, just outside) or anywhere in the bounds, straight
/// down, against the normal, grazing or any direction, from on the surface, behind it or
/// above.
fn mesh_case(r: &mut Rng, mesh: &Bv4Mesh, shape_pose: Transform, lossy: [f32; 3]) -> MeshCase {
    let pose = match r.below(4) {
        0 | 1 => shape_pose,
        2 => random_pose(r),
        _ => {
            // The collider's rotation with its zero components' signs flipped at random.
            let mut q = shape_pose.q;
            for c in [&mut q.x, &mut q.y, &mut q.z] {
                if *c == 0.0 && r.below(2) == 0 {
                    *c = -*c;
                }
            }
            if r.below(4) == 0 { Transform::IDENTITY } else { Transform { q, p: shape_pose.p } }
        }
    };
    let one = Vec3::new(1.0, 1.0, 1.0);
    let scale = match r.below(5) {
        0 | 1 => MeshScale { scale: one, rotation: Quat::IDENTITY },
        2 => MeshScale { scale: Vec3::from_array(lossy), rotation: Quat::IDENTITY },
        3 => {
            // Within a few ulps of one, as Unity's lossy scales of unscaled objects are.
            let near = |r: &mut Rng| f32::from_bits(1.0f32.to_bits() + r.below(9) as u32 - 4);
            MeshScale { scale: Vec3::new(near(r), near(r), near(r)), rotation: Quat::IDENTITY }
        }
        _ => {
            let s = |r: &mut Rng| r.range(0.3, 3.0) * if r.below(8) == 0 { -1.0 } else { 1.0 };
            let rotation = if r.below(3) == 0 { random_quat(r) } else { Quat::IDENTITY };
            MeshScale { scale: Vec3::new(s(r), s(r), s(r)), rotation }
        }
    };
    let to_world = |p: Vec3| pose.transform(scale.transform(p));
    let t = mesh.triangles[r.below(mesh.triangles.len() as u64) as usize];
    let [a, b, c] = t.map(|i| Vec3::from_array(mesh.vertices[i as usize]));
    let (u, v) = match r.below(6) {
        0 => (0.0, r.unit()),
        1 => (r.unit(), 0.0),
        2 => {
            let u = r.unit();
            (u, 1.0 - u)
        }
        3 => ([0.0, 1.0, 0.0][r.below(3) as usize], [0.0, 0.0, 1.0][r.below(3) as usize]),
        4 => (r.range(-1e-5, 1e-5), r.range(-1e-5, 1.0)),
        _ => {
            let (u, v) = (r.unit(), r.unit());
            if u + v > 1.0 { (1.0 - u, 1.0 - v) } else { (u, v) }
        }
    };
    let local = if r.below(8) == 0 {
        let c = Vec3::from_array(mesh.tree.center);
        let e = mesh.tree.extents_magnitude;
        Vec3::new(c.x + r.range(-e, e), c.y + r.range(-e, e), c.z + r.range(-e, e))
    } else {
        a.plus(b.minus(a).scale(u)).plus(c.minus(a).scale(v))
    };
    let target = to_world(local);
    let n = {
        let (wa, wb, wc) = (to_world(a), to_world(b), to_world(c));
        let n = wb.minus(wa).cross(wc.minus(wa));
        let m = n.magnitude();
        if m > 1e-12 { n.scale(1.0 / m) } else { Vec3::new(0.0, 1.0, 0.0) }
    };
    let dir = match r.below(6) {
        0 | 1 => Vec3::new(0.0, -1.0, 0.0),
        2 => n.negated(),
        3 => {
            let t = unit(r);
            let g = t.minus(n.scale(t.dot(n))).plus(n.scale(r.range(-1e-3, 1e-3)));
            let m = g.magnitude();
            if m > 1e-3 { g.scale(1.0 / m) } else { unit(r) }
        }
        _ => unit(r),
    };
    let back = match r.below(11) {
        0 | 1 => 0.0,
        2 | 3 => r.range(-1.0, 0.0),
        4 | 5 => r.range(0.0, 0.01),
        6 | 7 => r.range(0.0, 100.0),
        // Far away: f32 rounding of the order of the mesh (clipRay's own rounding).
        8 => 10f32.powf(r.range(5.0, 9.0)),
        _ => r.range(0.0, 5.0),
    };
    let origin = target.minus(dir.scale(back));
    let max = match r.below(6) {
        0 => 100.0,
        1 => r.range(0.0, 10.0),
        2 => 1.0e7,
        3 => f32::MAX,
        _ => r.range(0.0, 200.0),
    };
    MeshCase {
        scale,
        double_sided: r.below(8) == 0,
        pose,
        origin,
        dir,
        max,
        flags: if r.below(5) == 0 { 0x8b } else { 0xb },
        clip_to_hit: r.below(4) == 0,
    }
}

/// The area's mesh colliders (`wr-import phys-pack`): name, pose, lossy scale, mesh.
fn area_meshes() -> Vec<(String, Transform, [f32; 3], Bv4Mesh)> {
    let f = crate::PhysFile::read(&crate::path_for("bad465e5404604346bdf71ef50b3299a")).unwrap();
    f.world
        .shapes
        .into_iter()
        .filter_map(|s| match s.kind {
            crate::ShapeKind::Mesh { pose, lossy_scale, mesh, .. } => {
                Some((s.name, pose, lossy_scale, mesh))
            }
            _ => None,
        })
        .collect()
}

/// `n` generated queries per mesh collider through the player and the port: (queries, the
/// player's hits, differing, the first differences).
fn compare_meshes(p: &Player, n: usize, seed: u64) -> (usize, usize, usize, Vec<String>) {
    let (mut total, mut hits, mut differ) = (0usize, 0usize, 0usize);
    let mut bad = Vec::new();
    for (k, (name, shape_pose, lossy, mesh)) in area_meshes().iter().enumerate() {
        let native = NativeMesh::new(mesh);
        let mut r = Rng(seed ^ (k as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        for _ in 0..n {
            let c = mesh_case(&mut r, mesh, *shape_pose, *lossy);
            let ours = |max: f32| -> MeshOut {
                crate::bv4::raycast_triangle_mesh(
                    mesh,
                    &c.scale,
                    c.double_sided,
                    c.pose,
                    c.origin,
                    c.dir,
                    max,
                    c.flags,
                )
                .map(|h| (h.distance, h.position.to_array(), h.face_index, h.u, h.v))
            };
            let mut max = c.max;
            if c.clip_to_hit
                && let Some(h) = p.raycast_mesh(&native, &c, max)
            {
                max = h.0;
            }
            let want = p.raycast_mesh(&native, &c, max);
            let got = ours(max);
            total += 1;
            hits += usize::from(want.is_some());
            if mesh_key(want) != mesh_key(got) {
                differ += 1;
                if bad.len() < 20 {
                    bad.push(format!(
                        "{name}: scale {:?} sided {} pose {:?} from {:?} dir {:?} max {max} flags {:#x}: player {want:?}, ours {got:?}",
                        c.scale, c.double_sided, c.pose, c.origin, c.dir, c.flags
                    ));
                }
            }
        }
    }
    (total, hits, differ, bad)
}

fn env_count(var: &str, default: usize) -> usize {
    std::env::var(var).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

#[test]
#[ignore = "needs builds/arm64-oracle and the area's physics file (wr-import phys-pack)"]
fn raycast_triangle_mesh_matches_the_player() {
    let p = Player::load();
    let n = env_count("WR_PHYS_MESH_RAYS", 1_000_000);
    let (total, hits, differ, bad) = compare_meshes(&p, n, 0x5851_f42d_4c95_7f2d);
    eprintln!(
        "{total} rays on {} mesh colliders ({hits} hits), {differ} differ",
        area_meshes().len()
    );
    for b in &bad {
        eprintln!("  {b}");
    }
    assert!(hits > total / 10, "too few hits to say much");
    assert_eq!(differ, 0);
}

#[test]
#[ignore = "needs builds/arm64-oracle and the area's physics file (wr-import phys-pack)"]
fn the_mesh_check_catches_errors() {
    let p = Player::load();
    let n = env_count("WR_PHYS_MUTATION_RAYS", 20_000);
    let mut missed = Vec::new();
    for (m, what) in crate::bv4::mutation::ALL {
        crate::bv4::mutation::ACTIVE.with(|a| a.set(m));
        let (total, _, differ, _) = compare_meshes(&p, n, 0x2545_f491_4f6c_dd1d ^ u64::from(m));
        crate::bv4::mutation::ACTIVE.with(|a| a.set(0));
        eprintln!("mutation {m} ({what}): {differ} of {total} differ");
        if differ == 0 {
            missed.push(what);
        }
    }
    assert!(missed.is_empty(), "not caught: {missed:?}");
}

#[test]
#[ignore = "sweeps every f32 (about a minute)"]
fn frecpe_and_frecps_match_the_hardware() {
    #[cfg(target_arch = "aarch64")]
    {
        use std::arch::aarch64::{vrecpes_f32, vrecpss_f32};
        let same = |a: f32, b: f32| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
        let mut differ = 0u64;
        for bits in 0..=u32::MAX {
            let x = f32::from_bits(bits);
            // SAFETY: NEON is always present on aarch64.
            let want = unsafe { vrecpes_f32(x) };
            differ += u64::from(!same(want, crate::bv4::frecpe(x)));
        }
        assert_eq!(differ, 0, "frecpe differs on {differ} inputs");
        let mut r = Rng(0x1234_5678_9abc_def1);
        for _ in 0..10_000_000 {
            let a = f32::from_bits(r.next() as u32);
            let b = if r.below(2) == 0 {
                f32::from_bits(r.next() as u32)
            } else {
                crate::bv4::frecpe(a)
            };
            // SAFETY: as above.
            let want = unsafe { vrecpss_f32(a, b) };
            let got = crate::bv4::frecps(a, b);
            assert!(same(want, got), "frecps({a:e}, {b:e}): {want:e} vs {got:e}");
        }
    }
}

/// A float of any magnitude and sign: often a round or special value (zero of either sign, one,
/// a tiny or subnormal value), else spread over many binades.
fn any_float(r: &mut Rng) -> f32 {
    let sign = if r.below(2) == 0 { 1.0 } else { -1.0 };
    sign * match r.below(8) {
        0 => [0.0, 1.0, 0.5, f32::EPSILON, 1e-9, 1e-40, 0.257_168_6, 100.0][r.below(8) as usize],
        1 => r.range(0.0, 2.0e-7),
        _ => r.range(1.0, 2.0) * 2f32.powi(r.below(40) as i32 - 20),
    }
}

#[test]
#[ignore = "needs builds/arm64-oracle (the player's UnityPlayer.dylib)"]
fn transform_point_and_box_extents_match_the_player() {
    use crate::boxes::{LocalTrs, transform_point};
    let p = Player::load();
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let n = env_count("WR_PHYS_TRANSFORM_POINTS", 1_000_000);
    let mut bad = Vec::new();
    for _ in 0..n {
        let depth = 1 + r.below(5) as usize;
        let chain: Vec<LocalTrs> = (0..depth)
            .map(|_| {
                let q = match r.below(4) {
                    0 => Quat::IDENTITY,
                    1 => {
                        // About one axis (zeros in the other components).
                        let (s, c) = r.range(-3.2, 3.2).sin_cos();
                        let mut q = [0.0, 0.0, 0.0, c];
                        q[r.below(3) as usize] = s;
                        Quat { x: q[0], y: q[1], z: q[2], w: q[3] }
                    }
                    _ => random_quat(&mut r),
                };
                let scale = match r.below(4) {
                    0 => [1.0; 3],
                    1 => {
                        let s = any_float(&mut r);
                        [s; 3]
                    }
                    _ => [any_float(&mut r), any_float(&mut r), any_float(&mut r)],
                };
                let size = [1.0, 100.0, 10_000.0][r.below(3) as usize];
                LocalTrs {
                    position: [r.range(-size, size), r.range(-size, size), r.range(-size, size)],
                    rotation: [q.x, q.y, q.z, q.w],
                    scale,
                }
            })
            .collect();
        let pt = if r.below(4) == 0 {
            [0.0; 3]
        } else {
            [any_float(&mut r), any_float(&mut r), any_float(&mut r)]
        };
        let want = p.transform_point(&chain, pt);
        let got = transform_point(&chain, pt);
        let same = |a: [f32; 3], b: [f32; 3]| {
            a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()))
        };
        if !same(want, got) && bad.len() < 20 {
            bad.push(format!("TransformPoint {chain:?} {pt:?}: player {want:?}, ours {got:?}"));
        }
        let size = [any_float(&mut r), any_float(&mut r), any_float(&mut r)];
        let lossy = [any_float(&mut r), any_float(&mut r), any_float(&mut r)];
        let want = p.box_extents(size, lossy);
        let rt = crate::boxes::BoxRuntime {
            world_center: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            lossy_scale: lossy,
            position: [0.0; 3],
            layer: 0,
            source: String::new(),
        };
        let got = rt.half_extents(size).to_array();
        if !same(want, got) && bad.len() < 40 {
            bad.push(format!(
                "half extents of {size:?} under {lossy:?}: player {want:?}, ours {got:?}"
            ));
        }
    }
    for b in &bad {
        eprintln!("  {b}");
    }
    assert!(bad.is_empty(), "{} differ", bad.len());
    eprintln!("{n} TransformPoint calls and {n} box extents bit-identical");
}
