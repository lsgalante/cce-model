//! The light, baked into the vertices the raster pass draws.
//!
//! cce-ui's scene vertex is a position and a colour, nothing more. Its own
//! shading is flat, from screen-space derivatives, so a curved surface reads
//! as facets. A `prelit` draw skips that and shows the vertex colours as
//! they are, so smooth shading is a matter of lighting each corner here,
//! from its normal, before upload. The lights are fixed in world space (a
//! key, a fill and a sky/ground ambient), so the bake never changes as the
//! camera orbits: the model turns under a studio rig, as on a turntable.

use cce_mesh_io::Mesh;
use cce_ui::engine::Vertex3D;
use glam::Vec3;

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

/// The triangle list the raster pass draws, each corner lit.
pub fn bake(mesh: &Mesh) -> Vec<Vertex3D> {
    let normals = mesh.corner_normals(CREASE_DEGREES);
    let mut out = Vec::with_capacity(mesh.triangles.len() * 3);
    for (f, t) in mesh.triangles.iter().enumerate() {
        for (k, &v) in t.iter().enumerate() {
            let light = shade(normals[f * 3 + k]);
            out.push(Vertex3D {
                position: mesh.positions[v as usize].to_array(),
                color: mesh.corner_color(f, k).map(|c| (c * light).min(1.0)),
            });
        }
    }
    out
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

    /// A unit cube, as twelve triangles over eight shared points.
    fn cube(corner_colors: Option<Vec<[f32; 3]>>) -> Mesh {
        let p = |i: u32| Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32);
        let quads = [[1, 3, 7, 5], [4, 6, 2, 0], [2, 6, 7, 3], [4, 0, 1, 5], [4, 5, 7, 6], [1, 0, 2, 3]];
        Mesh {
            positions: (0..8).map(p).collect(),
            triangles: quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect(),
            tri_color: vec![0; 12],
            colors: vec![cce_mesh_io::CLAY],
            corner_colors,
        }
    }

    #[test]
    fn a_cube_bakes_to_six_flat_shades() {
        let verts = bake(&cube(None));
        assert_eq!(verts.len(), 36);
        // Its edges are past the crease, so each face is one shade from its
        // own normal, and no two faces are lit alike.
        let mut shades: Vec<u32> = verts.iter().map(|v| (v.color[0] * 1e4) as u32).collect();
        shades.sort();
        shades.dedup();
        assert_eq!(shades.len(), 6, "{shades:?}");
    }

    #[test]
    fn corner_colours_are_lit_as_they_are() {
        let verts = bake(&cube(Some(vec![[0.0, 0.5, 0.0]; 36])));
        assert!(verts.iter().all(|v| v.color[0] == 0.0 && v.color[1] > 0.0 && v.color[2] == 0.0));
    }

    #[test]
    fn no_face_is_left_black() {
        for n in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
            assert!(shade(n) >= GROUND, "{n} is darker than the ambient floor");
        }
        assert!(shade(KEY.normalize()) > 1.0 - 0.01, "the key-lit face should be near full");
    }
}
