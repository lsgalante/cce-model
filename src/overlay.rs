//! What is drawn over and under the model as lines: the grid floor, the
//! wireframe and the normals. Each is a list of vertex PAIRS for a wireframe
//! scene draw, in the fitted space the model is drawn in (the unit sphere).
//!
//! The grid is laid out in the FILE's units, so its lines fall on round
//! numbers of millimetres (or whatever the file is in) and one of them runs
//! through the file's origin; only then is it carried into the fitted space.

use std::collections::HashSet;

use cce_mesh_io::Mesh;
use cce_ui::engine::Vertex3D;
use glam::Vec3;

use crate::units;

/// The colours of the lines, linear RGB, chosen against the background
/// gradient: a minor grid line barely above it, every fifth a step brighter.
const GRID_MINOR: [f32; 3] = [0.075, 0.078, 0.088];
const GRID_MAJOR: [f32; 3] = [0.16, 0.165, 0.185];
const EDGE: [f32; 3] = [0.82, 0.84, 0.9];
const NORMAL_BASE: [f32; 3] = [0.15, 0.55, 0.85];
const NORMAL_TIP: [f32; 3] = [0.55, 0.9, 1.0];

/// About this many cells across the model's footprint.
const GRID_CELLS: f32 = 12.0;
/// The longest a normal is drawn, in the fitted space (the model is 2
/// across). A dense model's are shorter: see [`normals`].
const NORMAL_LENGTH: f32 = 0.04;
const NORMAL_LENGTH_MIN: f32 = 0.004;
/// Above this many triangles there is no wireframe: its edges outnumber
/// the window's pixels, so it paints the model solid (a 5M-triangle model's
/// was a white silhouette) and costs 360 MB doing it.
pub const MAX_WIRE_TRIANGLES: usize = 2_000_000;
/// More normals than this are thinned to every n-th: past it they are a
/// solid fur that hides the model they describe.
pub const MAX_NORMALS: usize = 50_000;

/// Where the fitted space sits in the file's: a file point p is drawn at
/// (p − centre) / radius.
#[derive(Debug, Clone, Copy)]
pub struct Fit {
    pub centre: Vec3,
    pub radius: f32,
    /// The model's box, in file units (after the turn upright).
    pub lo: Vec3,
    pub hi: Vec3,
}

impl Fit {
    fn place(&self, p: Vec3) -> [f32; 3] {
        ((p - self.centre) / self.radius).to_array()
    }
}

pub struct Grid {
    pub lines: Vec<Vertex3D>,
    /// One cell's side, in file units.
    pub step: f32,
}

/// A floor under the model: square, a little wider than its footprint,
/// at the height of its lowest point.
pub fn grid(fit: &Fit) -> Grid {
    let size = fit.hi - fit.lo;
    let footprint = size.x.max(size.z).max(fit.radius * 0.5);
    let step = units::nice_step(footprint / GRID_CELLS);
    let half = 0.75 * footprint + step;
    let (cx, cz) = ((fit.lo.x + fit.hi.x) / 2.0, (fit.lo.z + fit.hi.z) / 2.0);
    // Whole multiples of the step, so the lines sit on round numbers and
    // index 0 is the file's origin.
    let (x0, x1) = (((cx - half) / step).floor() as i64, ((cx + half) / step).ceil() as i64);
    let (z0, z1) = (((cz - half) / step).floor() as i64, ((cz + half) / step).ceil() as i64);
    let y = fit.lo.y;
    let colour = |k: i64| if k % 5 == 0 { GRID_MAJOR } else { GRID_MINOR };
    let mut lines = Vec::new();
    let mut line = |a: Vec3, b: Vec3, color: [f32; 3]| {
        lines.push(Vertex3D { position: fit.place(a), color });
        lines.push(Vertex3D { position: fit.place(b), color });
    };
    for k in x0..=x1 {
        let x = k as f32 * step;
        line(Vec3::new(x, y, z0 as f32 * step), Vec3::new(x, y, z1 as f32 * step), colour(k));
    }
    for k in z0..=z1 {
        let z = k as f32 * step;
        line(Vec3::new(x0 as f32 * step, y, z), Vec3::new(x1 as f32 * step, y, z), colour(k));
    }
    Grid { lines, step }
}

/// Every edge of every triangle, once; nothing past [`MAX_WIRE_TRIANGLES`].
pub fn edges(mesh: &Mesh) -> Vec<Vertex3D> {
    if mesh.triangles.len() > MAX_WIRE_TRIANGLES {
        return Vec::new();
    }
    let mut keys: Vec<u64> = Vec::with_capacity(mesh.triangles.len() * 3);
    for t in &mesh.triangles {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            keys.push(((a.min(b) as u64) << 32) | a.max(b) as u64);
        }
    }
    keys.sort_unstable();
    keys.dedup();
    let mut out = Vec::with_capacity(keys.len() * 2);
    for k in keys {
        for i in [(k >> 32) as usize, (k & 0xffff_ffff) as usize] {
            out.push(Vertex3D { position: mesh.positions[i].to_array(), color: EDGE });
        }
    }
    out
}

/// A short line out of each point along each normal it is lit with — one
/// per point on a smooth surface, one per face meeting at a crease — and
/// every n-th of them past [`MAX_NORMALS`].
///
/// Each is about as long as the gap between the normals drawn (the square
/// root of the surface's area over their number), up to [`NORMAL_LENGTH`]:
/// at a fixed length, a dense model's normals overlap into a fur that hides
/// the surface they describe.
pub fn normals(mesh: &Mesh, crease_degrees: f32) -> Vec<Vertex3D> {
    let corner = mesh.corner_normals(crease_degrees);
    let mut seen: HashSet<(u32, [i16; 3])> = HashSet::new();
    let mut picked: Vec<(Vec3, Vec3)> = Vec::new();
    for (t, tri) in mesh.triangles.iter().enumerate() {
        for (k, &v) in tri.iter().enumerate() {
            let n = corner[t * 3 + k];
            let q = (n * 1000.0).round();
            if seen.insert((v, [q.x as i16, q.y as i16, q.z as i16])) {
                picked.push((mesh.positions[v as usize], n));
            }
        }
    }
    let stride = picked.len().div_ceil(MAX_NORMALS).max(1);
    let area: f32 = mesh
        .triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            (b - a).cross(c - a).length() / 2.0
        })
        .sum();
    let shown = picked.len().div_ceil(stride).max(1) as f32;
    let length = (area / shown).sqrt().clamp(NORMAL_LENGTH_MIN, NORMAL_LENGTH);
    picked
        .iter()
        .step_by(stride)
        .flat_map(|&(p, n)| {
            [
                Vertex3D { position: p.to_array(), color: NORMAL_BASE },
                Vertex3D { position: (p + n * length).to_array(), color: NORMAL_TIP },
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Mesh {
        let p = |i: u32| Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32);
        let quads = [[1, 3, 7, 5], [4, 6, 2, 0], [2, 6, 7, 3], [4, 0, 1, 5], [4, 5, 7, 6], [1, 0, 2, 3]];
        Mesh {
            positions: (0..8).map(p).collect(),
            triangles: quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect(),
            tri_color: vec![0; 12],
            colors: vec![cce_mesh_io::CLAY],
            corner_colors: None,
        }
    }

    #[test]
    fn a_cube_has_eighteen_edges() {
        // Twelve sides and one diagonal across each of its six faces.
        assert_eq!(edges(&cube()).len(), 18 * 2);
    }

    #[test]
    fn a_cube_has_three_normals_a_corner() {
        // Every corner meets three faces at 90°, past the crease.
        assert_eq!(normals(&cube(), 40.0).len(), 8 * 3 * 2);
    }

    #[test]
    fn the_grid_is_on_round_numbers_under_the_model() {
        // A 120 × 30 × 80 part standing on z... after the turn: y is up.
        let fit = Fit { centre: Vec3::new(60.0, 15.0, 40.0), radius: 75.0, lo: Vec3::ZERO, hi: Vec3::new(120.0, 30.0, 80.0) };
        let g = grid(&fit);
        assert_eq!(g.step, 10.0, "120 mm across / 12 cells");
        for pair in g.lines.chunks(2) {
            let a = Vec3::from(pair[0].position) * fit.radius + fit.centre;
            assert!((a.y - 0.0).abs() < 1e-3, "the floor is at the model's lowest point");
            for c in [a.x, a.z] {
                assert!((c / g.step - (c / g.step).round()).abs() < 1e-3, "a line end off the grid: {c}");
            }
        }
        // The line through the file's origin is a major one.
        let origin_line = g.lines.chunks(2).find(|p| {
            let a = Vec3::from(p[0].position) * fit.radius + fit.centre;
            let b = Vec3::from(p[1].position) * fit.radius + fit.centre;
            a.x.abs() < 1e-3 && b.x.abs() < 1e-3
        });
        assert_eq!(origin_line.unwrap()[0].color, GRID_MAJOR);
    }

    #[test]
    fn too_many_normals_are_thinned() {
        let mut m = Mesh::default();
        let n = 300; // 90,000 points: past the cap, so every other one
        for i in 0..n {
            for j in 0..n {
                m.positions.push(Vec3::new(i as f32, 0.0, j as f32));
            }
        }
        for i in 0..n - 1 {
            for j in 0..n - 1 {
                let a = (i * n + j) as u32;
                m.triangles.push([a, a + 1, a + n as u32]);
                m.triangles.push([a + 1, a + n as u32 + 1, a + n as u32]);
            }
        }
        m.tri_color = vec![0; m.triangles.len()];
        let lines = normals(&m, 40.0).len() / 2;
        assert!(lines <= MAX_NORMALS && lines > MAX_NORMALS / 3, "{lines}");
    }
}
