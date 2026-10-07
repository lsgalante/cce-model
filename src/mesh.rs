//! A model as the viewer holds it: one welded triangle mesh with a colour
//! per triangle, and the vertices the raster pass draws, baked from it.
//!
//! ## Why the light is baked
//!
//! cce-ui's scene vertex is a position and a colour, nothing more. Its own
//! shading is flat, from screen-space derivatives, so a curved surface reads
//! as facets. A `prelit` draw skips that and shows the vertex colours as
//! they are, so smooth shading is a matter of lighting each corner here,
//! from its normal, before upload. The lights are fixed in world space (a
//! key, a fill and a sky/ground ambient), so the bake never changes as the
//! camera orbits: the model turns under a studio rig, as on a turntable.
//!
//! ## Why the mesh is welded
//!
//! An STL has no shared points: each triangle carries its own corners. A
//! smooth normal is the average of the faces meeting at a point, which
//! needs those faces to name the same point, so every loader's output is
//! welded by position before normals are taken.

use std::collections::HashMap;

use cce_ui::engine::Vertex3D;
use glam::Vec3;

/// The colour of a surface that names none (an STL; an OBJ without a
/// material), in the linear values the scene pass draws: a warm clay.
pub const CLAY: [f32; 3] = [0.42, 0.40, 0.36];

/// Faces meeting at a sharper angle than this keep separate normals, so a
/// cube's edges stay edges and a sphere's facets blend.
pub const CREASE_DEGREES: f32 = 40.0;

/// Direction TOWARD the key light, world space (Y up): above and to the
/// left of the home three-quarter view, so a model opens with a lit side
/// and a shaded side rather than lit flat from the camera.
pub const KEY: Vec3 = Vec3::new(-0.35, 0.75, 0.55);
const KEY_STRENGTH: f32 = 0.95;
/// Toward the fill: the home view's right, low, so the shaded side is not black.
const FILL: Vec3 = Vec3::new(0.75, 0.1, -0.1);
const FILL_STRENGTH: f32 = 0.25;
/// Ambient from above and below: a face turned to the sky is lit a little
/// more than one turned to the floor.
const SKY: f32 = 0.20;
const GROUND: f32 = 0.06;

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// One per triangle: an index into `colors`.
    pub tri_color: Vec<u32>,
    pub colors: Vec<[f32; 3]>,
}

impl Mesh {
    /// The axis-aligned box around every point, or `None` for no points.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let first = *self.positions.first()?;
        Some(self.positions.iter().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
    }

    /// Move and scale the mesh so its bounding box is centred on the origin
    /// and its farthest point is 1 from it, and return the (centre, radius)
    /// it had, so its real size can still be reported. The radius is the
    /// farthest point's, not the box's half-diagonal: a sphere's box corners
    /// sit 1.7 times farther out than any of its points, and framing those
    /// left a framed sphere small in the window.
    ///
    /// Not only for a tidy camera: cce-ui's scene pass reads any vertex
    /// whose z is within 0.01 of 9.99 as a corner of the screen-space
    /// background quad (`scene3d.wgsl`), whatever mesh it is in. A model
    /// spanning z = 9.99 in its own units had a ring of its points flung
    /// across the screen. Inside a unit sphere no point comes near it.
    pub fn fit_to_unit(&mut self) -> (Vec3, f32) {
        let Some((lo, hi)) = self.bounds() else { return (Vec3::ZERO, 1.0) };
        let centre = (lo + hi) / 2.0;
        let radius = self.positions.iter().map(|p| p.distance(centre)).fold(0.0, f32::max).max(f32::MIN_POSITIVE);
        for p in &mut self.positions {
            *p = (*p - centre) / radius;
        }
        (centre, radius)
    }

    /// Merge points that sit at the same place (to a millionth of the
    /// model's size) and drop the triangles that collapse doing so.
    pub fn weld(&mut self) {
        let Some((lo, hi)) = self.bounds() else { return };
        let cell = ((hi - lo).length() * 1e-6).max(f32::MIN_POSITIVE);
        let key = |p: Vec3| {
            let q = (p - lo) / cell;
            [q.x.round() as i64, q.y.round() as i64, q.z.round() as i64]
        };
        let mut index: HashMap<[i64; 3], u32> = HashMap::with_capacity(self.positions.len());
        let mut positions = Vec::with_capacity(self.positions.len());
        let remap: Vec<u32> = self
            .positions
            .iter()
            .map(|p| {
                *index.entry(key(*p)).or_insert_with(|| {
                    positions.push(*p);
                    (positions.len() - 1) as u32
                })
            })
            .collect();
        let mut triangles = Vec::with_capacity(self.triangles.len());
        let mut tri_color = Vec::with_capacity(self.triangles.len());
        for (t, c) in self.triangles.iter().zip(&self.tri_color) {
            let [a, b, c3] = t.map(|i| remap[i as usize]);
            if a != b && b != c3 && a != c3 {
                triangles.push([a, b, c3]);
                tri_color.push(*c);
            }
        }
        self.positions = positions;
        self.triangles = triangles;
        self.tri_color = tri_color;
    }

    /// A normal for each triangle corner (three per triangle, in order):
    /// the area-weighted average of the faces at that point which meet this
    /// triangle within the crease angle.
    pub fn corner_normals(&self, crease_degrees: f32) -> Vec<Vec3> {
        let crease_cos = crease_degrees.to_radians().cos();
        // The cross product's length is twice the area, so summing raw
        // crosses weights each face by its area.
        let cross: Vec<Vec3> = self
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.positions[i as usize]);
                (b - a).cross(c - a)
            })
            .collect();
        let unit: Vec<Vec3> = cross.iter().map(|n| n.normalize_or_zero()).collect();

        // Faces at each point, as one flat list with offsets.
        let mut start = vec![0u32; self.positions.len() + 1];
        for t in &self.triangles {
            for &v in t {
                start[v as usize + 1] += 1;
            }
        }
        for i in 1..start.len() {
            start[i] += start[i - 1];
        }
        let mut fill = start.clone();
        let mut faces = vec![0u32; self.triangles.len() * 3];
        for (f, t) in self.triangles.iter().enumerate() {
            for &v in t {
                faces[fill[v as usize] as usize] = f as u32;
                fill[v as usize] += 1;
            }
        }

        let mut out = Vec::with_capacity(self.triangles.len() * 3);
        for (f, t) in self.triangles.iter().enumerate() {
            for &v in t {
                let around = &faces[start[v as usize] as usize..start[v as usize + 1] as usize];
                let sum: Vec3 = around
                    .iter()
                    .filter(|&&g| unit[g as usize].dot(unit[f]) >= crease_cos)
                    .map(|&g| cross[g as usize])
                    .sum();
                out.push(sum.try_normalize().unwrap_or(unit[f]));
            }
        }
        out
    }

    /// The triangle list the raster pass draws, each corner lit.
    pub fn bake(&self) -> Vec<Vertex3D> {
        let normals = self.corner_normals(CREASE_DEGREES);
        let mut out = Vec::with_capacity(self.triangles.len() * 3);
        for (f, t) in self.triangles.iter().enumerate() {
            let albedo = self.colors.get(self.tri_color[f] as usize).copied().unwrap_or(CLAY);
            for (k, &v) in t.iter().enumerate() {
                let light = shade(normals[f * 3 + k]);
                out.push(Vertex3D {
                    position: self.positions[v as usize].to_array(),
                    color: albedo.map(|c| (c * light).min(1.0)),
                });
            }
        }
        out
    }
}

/// How much light reaches a surface facing `n`, as a multiplier on its
/// albedo.
pub fn shade(n: Vec3) -> f32 {
    let ambient = GROUND + (SKY - GROUND) * (0.5 + 0.5 * n.y);
    let key = KEY_STRENGTH * n.dot(KEY.normalize()).max(0.0);
    let fill = FILL_STRENGTH * n.dot(FILL.normalize()).max(0.0);
    ambient + key + fill
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube as an STL would carry it: twelve triangles, every corner
    /// its own point.
    pub(crate) fn soup_cube() -> Mesh {
        let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let quads = [
            [c(1., 0., 0.), c(1., 1., 0.), c(1., 1., 1.), c(1., 0., 1.)],
            [c(0., 0., 1.), c(0., 1., 1.), c(0., 1., 0.), c(0., 0., 0.)],
            [c(0., 1., 0.), c(0., 1., 1.), c(1., 1., 1.), c(1., 1., 0.)],
            [c(0., 0., 1.), c(0., 0., 0.), c(1., 0., 0.), c(1., 0., 1.)],
            [c(0., 0., 1.), c(1., 0., 1.), c(1., 1., 1.), c(0., 1., 1.)],
            [c(1., 0., 0.), c(0., 0., 0.), c(0., 1., 0.), c(1., 1., 0.)],
        ];
        let mut m = Mesh::default();
        for q in quads {
            for i in [0, 1, 2, 0, 2, 3] {
                m.positions.push(q[i]);
            }
        }
        m.triangles = (0..12).map(|t| [t * 3, t * 3 + 1, t * 3 + 2]).collect();
        m.tri_color = vec![0; 12];
        m.colors = vec![CLAY];
        m
    }

    #[test]
    fn welding_a_cube_leaves_its_eight_corners() {
        let mut m = soup_cube();
        assert_eq!(m.positions.len(), 36);
        m.weld();
        assert_eq!(m.positions.len(), 8);
        assert_eq!(m.triangles.len(), 12);
    }

    #[test]
    fn a_cube_keeps_its_edges_and_its_faces_point_out() {
        let mut m = soup_cube();
        m.weld();
        let normals = m.corner_normals(CREASE_DEGREES);
        let (lo, hi) = m.bounds().unwrap();
        let center = (lo + hi) / 2.0;
        for (f, t) in m.triangles.iter().enumerate() {
            let [a, b, c] = t.map(|i| m.positions[i as usize]);
            let face = (b - a).cross(c - a).normalize();
            assert!(face.dot((a + b + c) / 3.0 - center) > 0.0, "triangle {f} faces inward");
            for k in 0..3 {
                // 90° edges are past the crease: every corner keeps its face's normal.
                assert!(normals[f * 3 + k].dot(face) > 0.999, "triangle {f} corner {k} was smoothed");
            }
        }
    }

    #[test]
    fn a_shallow_fold_is_smoothed() {
        // Two triangles meeting at 20°: one shared normal along the fold.
        let mut m = Mesh::default();
        let lift = 20f32.to_radians().tan();
        m.positions = vec![Vec3::ZERO, Vec3::Z, Vec3::new(-1.0, 0.0, 0.5), Vec3::new(1.0, lift, 0.5)];
        m.triangles = vec![[0, 1, 2], [0, 3, 1]];
        m.tri_color = vec![0, 0];
        let n = m.corner_normals(CREASE_DEGREES);
        assert!(n[0].dot(n[3]) > 0.9999, "the shared point has two normals");
    }

    #[test]
    fn welding_drops_a_collapsed_triangle() {
        let mut m = Mesh::default();
        m.positions = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::X, Vec3::X * (1.0 + 1e-9), Vec3::Y];
        m.triangles = vec![[0, 1, 2], [3, 4, 5]];
        m.tri_color = vec![0, 0];
        m.weld();
        assert_eq!(m.triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn a_fitted_mesh_stays_clear_of_the_background_sentinel() {
        // The sphere that showed the bug: z from 0 to 25.
        let mut m = soup_cube();
        for p in &mut m.positions {
            *p = *p * 25.0 + Vec3::new(-12.5, -12.5, 0.0);
        }
        let (centre, radius) = m.fit_to_unit();
        assert_eq!(centre, Vec3::new(0.0, 0.0, 12.5));
        assert!((radius - 25.0 * 3f32.sqrt() / 2.0).abs() < 1e-3, "a cube's corners are its farthest points");
        assert!(m.positions.iter().all(|p| p.length() <= 1.0 + 1e-5));
    }

    #[test]
    fn no_face_is_left_black() {
        for n in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
            assert!(shade(n) >= GROUND, "{n} is darker than the ambient floor");
        }
        assert!(shade(KEY.normalize()) > 1.0 - 0.01, "the key-lit face should be near full");
    }
}

#[cfg(test)]
pub(crate) use tests::soup_cube;
