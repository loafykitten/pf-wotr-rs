//! Scene hierarchy and world transforms, following Unity's conventions: left-handed,
//! Y up, `localToWorld = parent.localToWorld * T * R * S`, rotations as quaternions,
//! all in `f32` as Unity stores them.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use serde::Serialize;

/// A local transform as stored on a `Transform` component.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trs {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Trs {
    pub const IDENTITY: Self =
        Self { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };

    #[must_use]
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

/// A node of the hierarchy: its local transform and its parent's key.
#[derive(Debug, Clone)]
pub struct Node<K> {
    pub local: Trs,
    pub parent: Option<K>,
}

/// World-space result for one node.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct World {
    pub position: [f32; 3],
    /// Quaternion `x, y, z, w`: the product of the chain's local rotations
    /// (`Transform.rotation`).
    pub rotation: [f32; 4],
    /// `Transform.lossyScale` approximation: the scale part of the world matrix with the
    /// world rotation removed.
    pub lossy_scale: [f32; 3],
    /// Column-major `localToWorldMatrix`.
    pub matrix: [f32; 16],
    /// Depth in the hierarchy (roots are 0).
    pub depth: u32,
}

/// Error for a parent chain that loops or points at a missing node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HierarchyError<K> {
    Cycle(K),
    MissingParent { child: K, parent: K },
}

/// Computes world transforms for every node, memoising along parent chains.
///
/// # Errors
///
/// Returns the first cycle or dangling parent found; other nodes are still computed.
pub fn world_transforms<K>(
    nodes: &HashMap<K, Node<K>>,
) -> (HashMap<K, World>, Vec<HierarchyError<K>>)
where
    K: Copy + Eq + std::hash::Hash + Ord,
{
    let mut out: HashMap<K, (Mat4, Quat, u32)> = HashMap::with_capacity(nodes.len());
    let mut errors = Vec::new();
    let mut keys: Vec<K> = nodes.keys().copied().collect();
    keys.sort();
    for &k in &keys {
        if out.contains_key(&k) {
            continue;
        }
        // Walk up until a computed node or a root, then compose downwards.
        let mut chain = vec![k];
        let mut base: Option<(Mat4, Quat, u32)> = None;
        let mut failed = false;
        loop {
            let cur = *chain.last().expect("chain is never empty");
            let Some(parent) = nodes[&cur].parent else { break };
            if let Some(&w) = out.get(&parent) {
                base = Some(w);
                break;
            }
            if !nodes.contains_key(&parent) {
                errors.push(HierarchyError::MissingParent { child: cur, parent });
                break;
            }
            if chain.contains(&parent) {
                errors.push(HierarchyError::Cycle(parent));
                failed = true;
                break;
            }
            chain.push(parent);
        }
        if failed {
            continue;
        }
        // The topmost chain element sits one below `base`, or is a root.
        let (mut m, mut q, mut depth) =
            base.map_or((Mat4::IDENTITY, Quat::IDENTITY, 0), |(m, q, d)| (m, q, d + 1));
        for &n in chain.iter().rev() {
            let local = nodes[&n].local;
            m *= local.matrix();
            q *= local.rotation;
            out.insert(n, (m, q, depth));
            depth += 1;
        }
    }
    let worlds = out
        .into_iter()
        .map(|(k, (m, q, depth))| {
            let q = q.normalize();
            // Remove rotation from the 3x3 part to get the (possibly skewed) scale.
            let rs = glam::Mat3::from_mat4(m);
            let unrot = glam::Mat3::from_quat(q.inverse()) * rs;
            let lossy = Vec3::new(unrot.x_axis.x, unrot.y_axis.y, unrot.z_axis.z);
            let pos = m.w_axis.truncate();
            (
                k,
                World {
                    position: pos.to_array(),
                    rotation: q.to_array(),
                    lossy_scale: lossy.to_array(),
                    matrix: m.to_cols_array(),
                    depth,
                },
            )
        })
        .collect();
    (worlds, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn composes_parent_chain_like_unity() {
        let mut nodes = HashMap::new();
        // Root at (10,0,0), rotated 90° about Y, scaled 2.
        nodes.insert(
            1,
            Node {
                local: Trs {
                    position: Vec3::new(10.0, 0.0, 0.0),
                    rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                    scale: Vec3::splat(2.0),
                },
                parent: None,
            },
        );
        // Child one unit along local +X.
        nodes
            .insert(2, Node { local: Trs { position: Vec3::X, ..Trs::IDENTITY }, parent: Some(1) });
        // Grandchild one unit along local +Z.
        nodes
            .insert(3, Node { local: Trs { position: Vec3::Z, ..Trs::IDENTITY }, parent: Some(2) });
        let (w, errs) = world_transforms(&nodes);
        assert!(errs.is_empty());
        // Unity: rotating +X by 90° about Y gives -Z; scaled by 2.
        assert!(close(w[&2].position, [10.0, 0.0, -2.0]));
        // +Z rotated 90° about Y gives +X.
        assert!(close(w[&3].position, [12.0, 0.0, -2.0]));
        assert!(close(w[&3].lossy_scale, [2.0, 2.0, 2.0]));
        assert_eq!((w[&1].depth, w[&2].depth, w[&3].depth), (0, 1, 2));
    }

    #[test]
    fn order_of_evaluation_does_not_matter() {
        // Insert children before parents, with keys that sort child-first.
        let mut nodes = HashMap::new();
        nodes
            .insert(1, Node { local: Trs { position: Vec3::Y, ..Trs::IDENTITY }, parent: Some(5) });
        nodes
            .insert(0, Node { local: Trs { position: Vec3::X, ..Trs::IDENTITY }, parent: Some(1) });
        nodes.insert(5, Node { local: Trs { position: Vec3::Z, ..Trs::IDENTITY }, parent: None });
        let (w, _) = world_transforms(&nodes);
        assert!(close(w[&0].position, [1.0, 1.0, 1.0]));
        assert_eq!(w[&0].depth, 2);
        assert_eq!(w[&1].depth, 1);
    }

    #[test]
    fn reports_cycles_and_dangling_parents() {
        let mut nodes = HashMap::new();
        nodes.insert(1, Node { local: Trs::IDENTITY, parent: Some(2) });
        nodes.insert(2, Node { local: Trs::IDENTITY, parent: Some(1) });
        nodes.insert(3, Node { local: Trs::IDENTITY, parent: Some(9) });
        let (w, errs) = world_transforms(&nodes);
        assert!(errs.iter().any(|e| matches!(e, HierarchyError::Cycle(_))));
        assert!(errs.contains(&HierarchyError::MissingParent { child: 3, parent: 9 }));
        // A dangling parent is treated as a root.
        assert!(w.contains_key(&3));
    }
}
