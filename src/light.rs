//! The light, and the lit meshes it falls on.
//!
//! The model is drawn through cce-ui's lit pipeline (`draw::lit`): a vertex
//! with a normal, a texture coordinate and a colour, shaded per fragment
//! with a metallic-roughness specular, so highlights move with the camera
//! and textures show. This module turns a mesh into those vertices, one lit
//! mesh per material (a draw binds one texture), and holds the studio rig
//! they are lit by: a key, a fill and a sky/ground ambient, fixed in world
//! space, with the traced view's sun on the same key.

use cce_mesh_io::{Material, Mesh};
use cce_ui::engine::{LitLight, LitVertex};
use glam::Vec3;

/// Faces meeting at a sharper angle than this keep separate normals, so a
/// cube's edges stay edges and a sphere's facets blend. Used when the file
/// brings no normals of its own.
pub const CREASE_DEGREES: f32 = 40.0;

/// Direction TOWARD the key light, world space (Y up): above and to the
/// left of the home three-quarter view, so a model opens with a lit side
/// and a shaded side rather than lit flat from the camera.
pub const KEY: Vec3 = Vec3::new(-0.35, 0.75, 0.55);
/// Toward the fill: the home view's right, low, so the shaded side is not black.
const FILL: Vec3 = Vec3::new(0.75, 0.1, -0.1);

/// The rig the lit pipeline shades by.
pub fn rig() -> LitLight {
    LitLight {
        key_toward: KEY.to_array(),
        key_color: [0.95; 3],
        fill_toward: FILL.to_array(),
        fill_color: [0.25; 3],
        sky: [0.20; 3],
        ground: [0.06; 3],
    }
}

/// A normal per triangle corner: the file's own when it has them, else
/// derived with the crease angle.
pub fn normals(mesh: &Mesh) -> Vec<Vec3> {
    mesh.file_normals.clone().unwrap_or_else(|| mesh.corner_normals(CREASE_DEGREES))
}

/// The triangles of one material, as lit vertices.
#[derive(Debug)]
pub struct LitPart {
    pub verts: Vec<LitVertex>,
    pub material: Material,
}

/// The mesh as one lit part per material it uses. A mesh with corner colours
/// carries them on the vertices and draws its parts in white (the reader has
/// already folded the material's colour into them); one without draws white
/// vertices in its material's colour.
pub fn lit_parts(mesh: &Mesh) -> Vec<LitPart> {
    let normals = normals(mesh);
    let tinted = mesh.corner_colors.is_some();
    let mut parts: Vec<Option<LitPart>> = (0..mesh.materials.len().max(1)).map(|_| None).collect();
    for (t, tri) in mesh.triangles.iter().enumerate() {
        let m = (mesh.tri_material[t] as usize).min(parts.len() - 1);
        let part = parts[m].get_or_insert_with(|| {
            let mut material = mesh.material(t);
            if tinted {
                material.color = [1.0; 3];
            }
            LitPart { verts: Vec::new(), material }
        });
        for (k, &v) in tri.iter().enumerate() {
            let c = t * 3 + k;
            part.verts.push(LitVertex {
                position: mesh.positions[v as usize].to_array(),
                normal: normals[c].to_array(),
                uv: mesh.corner_uvs.as_ref().map_or([0.0; 2], |uv| uv[c]),
                color: if tinted { mesh.corner_color(t, k) } else { [1.0; 3] },
            });
        }
    }
    parts.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube, as twelve triangles over eight shared points.
    pub(crate) fn cube() -> Mesh {
        let p = |i: u32| Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32);
        let quads = [[1, 3, 7, 5], [4, 6, 2, 0], [2, 6, 7, 3], [4, 0, 1, 5], [4, 5, 7, 6], [1, 0, 2, 3]];
        Mesh {
            positions: (0..8).map(p).collect(),
            triangles: quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect(),
            tri_material: vec![0; 12],
            materials: vec![Material::default()],
            ..Mesh::default()
        }
    }

    #[test]
    fn a_cube_is_one_part_with_its_edges_kept() {
        let parts = lit_parts(&cube());
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].verts.len(), 36);
        assert_eq!(parts[0].material.color, cce_mesh_io::CLAY);
        // Past the crease: each face's corners carry that face's normal, so
        // six distinct normals in all.
        let mut normals: Vec<[i32; 3]> = parts[0].verts.iter().map(|v| v.normal.map(|c| c.round() as i32)).collect();
        normals.sort();
        normals.dedup();
        assert_eq!(normals.len(), 6);
    }

    #[test]
    fn each_material_is_its_own_part_and_corner_colours_ride_the_vertices() {
        let mut m = cube();
        m.materials.push(Material::colour([0.0, 0.0, 1.0]));
        m.tri_material[0] = 1;
        assert_eq!(lit_parts(&m).len(), 2);
        m.corner_colors = Some(vec![[0.0, 0.5, 0.0]; 36]);
        let parts = lit_parts(&m);
        assert!(parts.iter().all(|p| p.material.color == [1.0; 3]), "the colour is on the vertices");
        assert_eq!(parts[0].verts[0].color, [0.0, 0.5, 0.0]);
    }

    #[test]
    fn the_files_normals_win() {
        let mut m = cube();
        m.file_normals = Some(vec![Vec3::Y; 36]);
        assert!(lit_parts(&m)[0].verts.iter().all(|v| v.normal == [0.0, 1.0, 0.0]));
    }
}
