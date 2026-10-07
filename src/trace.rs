//! The path-traced view's scene, and how long to let it refine.
//!
//! cce-ui's tracer takes triangles with a material each (albedo and
//! emission, linear) in the space the camera's inverse mvp unprojects into —
//! the fitted space the raster pass draws in, so one camera serves both.
//! The model goes in with a colour per triangle (its corner colours
//! averaged, or its palette colour), deduplicated into materials, over a
//! wide ground plane at the grid's height, so the traced model stands on
//! something and casts a shadow; the grid's lines, which are not surfaces,
//! have no place in it.

use std::collections::HashMap;
use std::path::Path;

use cce_mesh_io::Mesh;
use cce_ui::engine::{RtEnvironment, RtMaterial, RtTriangle};

/// Samples to refine to: enough for the tracer's denoiser to settle, and
/// then the GPU is left alone. Fewer on battery, where every frame of
/// compute is drawn from it.
pub const SAMPLES_ON_MAINS: u32 = 256;
pub const SAMPLES_ON_BATTERY: u32 = 32;

/// The ground's half-width, in the fitted space (the model is 2 across):
/// far enough that its edge is past the horizon from any framing.
const GROUND_HALF: f32 = 60.0;
/// Dark, near the raster background, so switching views does not turn the
/// whole window grey.
const GROUND_ALBEDO: [f32; 3] = [0.09, 0.09, 0.095];

/// Corner colours are quantized this finely before they are shared as
/// materials, so a vertex-coloured scan does not become a material a
/// triangle.
const COLOUR_STEPS: f32 = 64.0;

/// What a camera ray that misses everything shows: the middle of the
/// raster view's background gradient, so the switch does not flash.
pub const BACKGROUND: [f32; 3] = [0.06, 0.063, 0.072];

/// The sky and sun, with the sun where the raster bake's key light is, and
/// the sky grey rather than the tracer's default blue: the model's own
/// colours should not shift when the view is switched.
pub fn environment(key: [f32; 3]) -> RtEnvironment {
    RtEnvironment {
        sun_direction: key,
        sky_zenith: [0.62, 0.63, 0.66],
        sky_nadir: [0.2, 0.2, 0.21],
        ..RtEnvironment::default()
    }
}

/// The model's triangles and a ground plane under them at `floor_y`.
pub fn scene(mesh: &Mesh, floor_y: f32) -> (Vec<RtTriangle>, Vec<RtMaterial>) {
    let mut materials: Vec<RtMaterial> = Vec::new();
    let mut slot: HashMap<[u16; 3], u32> = HashMap::new();
    let mut material_for = |c: [f32; 3]| -> u32 {
        let key = c.map(|v| (v.clamp(0.0, 1.0) * COLOUR_STEPS).round() as u16);
        *slot.entry(key).or_insert_with(|| {
            materials.push(RtMaterial { albedo: key.map(|k| k as f32 / COLOUR_STEPS), emission: [0.0; 3] });
            (materials.len() - 1) as u32
        })
    };
    let mut triangles = Vec::with_capacity(mesh.triangles.len() + 2);
    for (t, tri) in mesh.triangles.iter().enumerate() {
        let colour = if mesh.corner_colors.is_some() {
            let [a, b, c] = [0, 1, 2].map(|k| mesh.corner_color(t, k));
            [0, 1, 2].map(|i| (a[i] + b[i] + c[i]) / 3.0)
        } else {
            mesh.corner_color(t, 0)
        };
        let [p0, p1, p2] = tri.map(|i| mesh.positions[i as usize].to_array());
        triangles.push(RtTriangle { p0, p1, p2, material: material_for(colour) });
    }
    let ground = material_for(GROUND_ALBEDO);
    let g = GROUND_HALF;
    // Counter-clockwise seen from above.
    let corner = |x: f32, z: f32| [x, floor_y, z];
    triangles.push(RtTriangle { p0: corner(-g, -g), p1: corner(-g, g), p2: corner(g, g), material: ground });
    triangles.push(RtTriangle { p0: corner(-g, -g), p1: corner(g, g), p2: corner(g, -g), material: ground });
    (triangles, materials)
}

/// True when the machine is running from its battery: a mains supply is
/// listed under `root` (`/sys/class/power_supply`) and none is online. A
/// machine with no mains supply listed (a desktop) is never on battery.
pub fn on_battery(root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else { return false };
    let mut mains = false;
    for e in entries.flatten() {
        let read = |name: &str| std::fs::read_to_string(e.path().join(name)).unwrap_or_default();
        if read("type").trim() == "Mains" {
            mains = true;
            if read("online").trim() == "1" {
                return false;
            }
        }
    }
    mains
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn quad(corner_colors: Option<Vec<[f32; 3]>>) -> Mesh {
        Mesh {
            positions: vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            tri_color: vec![0, 0],
            colors: vec![[0.5, 0.25, 0.0]],
            corner_colors,
        }
    }

    #[test]
    fn the_model_stands_on_a_ground_plane() {
        let (tris, mats) = scene(&quad(None), -1.0);
        assert_eq!(tris.len(), 2 + 2);
        assert_eq!(mats.len(), 2, "one model colour and the ground");
        let ground = &tris[2];
        assert!([ground.p0, ground.p1, ground.p2].iter().all(|p| p[1] == -1.0));
        assert_eq!(mats[ground.material as usize].albedo, [0.09375, 0.09375, 0.09375], "the ground's colour, quantized");
        // Its normal points up, so it is lit from above.
        let [a, b, c] = [ground.p0, ground.p1, ground.p2].map(Vec3::from);
        assert!((b - a).cross(c - a).y > 0.0);
    }

    #[test]
    fn corner_colours_are_averaged_and_shared() {
        let red = [1.0, 0.0, 0.0];
        let (tris, mats) = scene(&quad(Some(vec![red; 6])), 0.0);
        assert_eq!(tris[0].material, tris[1].material, "two red triangles share one material");
        assert_eq!(mats[tris[0].material as usize].albedo, red);
    }

    #[test]
    fn mains_online_is_not_battery() {
        let root = std::env::temp_dir().join(format!("cce-model-power-{}", std::process::id()));
        let supply = |name: &str, ty: &str, online: Option<&str>| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("type"), format!("{ty}\n")).unwrap();
            if let Some(o) = online {
                std::fs::write(d.join("online"), format!("{o}\n")).unwrap();
            }
        };
        assert!(!on_battery(&root), "nothing listed: a desktop");
        supply("BAT0", "Battery", None);
        assert!(!on_battery(&root), "a battery and no mains supply listed");
        supply("AC", "Mains", Some("0"));
        assert!(on_battery(&root));
        supply("AC", "Mains", Some("1"));
        assert!(!on_battery(&root));
        std::fs::remove_dir_all(&root).ok();
    }
}
