//! Shader constants by name, as Unity sets them: globals (`Shader.SetGlobal*`), material
//! properties and per-draw built-ins. Constant buffers are filled member by member from
//! the layouts recorded in the pack, so each shader gets its own HLSLcc struct layout.

use std::collections::{BTreeSet, HashMap};

use bevy::math::{Mat4, Vec3, Vec4};
use wr_assets::CBufferLayout;

/// Name → 32-bit words (floats as bits; ints stored as their bits).
#[derive(Debug, Clone, Default)]
pub struct Consts(pub HashMap<String, Vec<u32>>);

impl Consts {
    pub fn f(&mut self, name: &str, v: &[f32]) {
        self.0.insert(name.to_owned(), v.iter().map(|x| x.to_bits()).collect());
    }

    pub fn v4(&mut self, name: &str, v: Vec4) {
        self.f(name, &v.to_array());
    }

    pub fn v3(&mut self, name: &str, v: Vec3) {
        self.f(name, &v.to_array());
    }

    pub fn i(&mut self, name: &str, v: &[i32]) {
        self.0.insert(name.to_owned(), v.iter().map(|x| *x as u32).collect());
    }

    /// A matrix as HLSLcc names it (`hlslcc_mtx4x4<name>`, four column vectors).
    pub fn m(&mut self, name: &str, m: Mat4) {
        self.f(&format!("hlslcc_mtx4x4{name}"), &m.to_cols_array());
    }
}

/// Fills a constant buffer: each member from the first source that has it, else zero.
/// Members nobody provided are added to `missing` (for diagnostics).
pub fn fill(
    layout: &CBufferLayout,
    sources: &[&Consts],
    missing: &mut BTreeSet<String>,
) -> Vec<u8> {
    let mut buf = vec![0u8; layout.size.max(16) as usize];
    for m in &layout.members {
        let Some(words) = sources.iter().find_map(|s| s.0.get(&m.name)) else {
            missing.insert(format!("{}.{}", layout.ty, m.name));
            continue;
        };
        // Arrays of vectors are 16-byte strided (float4 arrays); scalars/vectors pack.
        let start = m.offset as usize;
        let end = (m.offset + m.size) as usize;
        for (k, w) in words.iter().enumerate() {
            let o = start + k * 4;
            if o + 4 > end || o + 4 > buf.len() {
                break;
            }
            buf[o..o + 4].copy_from_slice(&w.to_le_bytes());
        }
    }
    buf
}

/// Unity's `Mathf.GammaToLinearSpace` (exact sRGB curve).
#[must_use]
pub fn gamma_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else if c < 1.0 {
        ((c + 0.055) / 1.055).powf(2.4)
    } else {
        c.powf(2.2)
    }
}

/// A colour converted to linear (alpha untouched), as Unity does for colour properties
/// and `Color.linear` in a linear-space project.
#[must_use]
pub fn linear(c: [f32; 4]) -> [f32; 4] {
    [gamma_to_linear(c[0]), gamma_to_linear(c[1]), gamma_to_linear(c[2]), c[3]]
}

/// A row-major `[[f32; 4]; 4]` from the route JSON as a glam matrix.
#[must_use]
pub fn mat_rows(r: [[f32; 4]; 4]) -> Mat4 {
    Mat4::from_cols_array_2d(&r).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wr_assets::Member;

    #[test]
    fn fill_by_member_name() {
        let layout = CBufferLayout {
            ty: "T".into(),
            size: 48,
            members: vec![
                Member { name: "a".into(), ty: "float".into(), count: 1, offset: 0, size: 4 },
                Member { name: "b".into(), ty: "float4".into(), count: 2, offset: 16, size: 32 },
            ],
        };
        let mut c = Consts::default();
        c.f("a", &[1.0, 9.0]);
        c.f("b", &[2.0; 8]);
        let mut miss = BTreeSet::new();
        let buf = fill(&layout, &[&c], &mut miss);
        assert_eq!(&buf[0..4], &1.0f32.to_le_bytes());
        assert_eq!(&buf[4..8], &[0; 4], "a's second word must not spill");
        assert_eq!(&buf[44..48], &2.0f32.to_le_bytes());
        assert!(miss.is_empty());
        let empty = Consts::default();
        fill(&layout, &[&empty], &mut miss);
        assert_eq!(miss.len(), 2);
    }

    #[test]
    fn srgb_curve() {
        assert!((gamma_to_linear(0.5) - 0.214_041_14).abs() < 1e-6);
        assert_eq!(gamma_to_linear(0.0), 0.0);
        assert!((gamma_to_linear(1.0) - 1.0).abs() < 1e-6);
        let m = mat_rows([
            [1.0, 2.0, 3.0, 4.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        assert_eq!(m.w_axis.x, 4.0);
        assert_eq!(m.y_axis.x, 2.0);
    }
}
