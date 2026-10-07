//! The orbit camera: an eye on a sphere around a pivot, looking at it.
//!
//! Yaw turns about the world's up (Y), pitch tilts toward the poles and
//! stops just short of them, where the up vector would flip and the view
//! would roll. Distance is how far the eye sits from the pivot. `radius` is
//! the size of what is being looked at; it sets the clip planes and the
//! zoom range, so a 2 mm part and a 200 m building handle alike.

use glam::{Mat4, Vec3};

/// Vertical field of view.
pub const FOV_Y_DEGREES: f32 = 35.0;
/// Radians of orbit per logical px of drag or scroll.
const ORBIT_RAD_PER_PX: f32 = 0.006;
const MAX_PITCH: f32 = 89.0 * std::f32::consts::PI / 180.0;
/// The three-quarter view a model opens in.
const HOME_YAW: f32 = 35.0 * std::f32::consts::PI / 180.0;
const HOME_PITCH: f32 = 22.0 * std::f32::consts::PI / 180.0;
/// How far from the pivot, in model radii, the clip planes keep the scene.
const CLIP_RADII: f32 = 3.0;
/// Room left around a framed model, as a share of its size.
const FRAME_MARGIN: f32 = 1.3;

#[derive(Debug, Clone)]
pub struct Camera {
    pub pivot: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub radius: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { pivot: Vec3::ZERO, yaw: HOME_YAW, pitch: HOME_PITCH, distance: 4.0, radius: 1.0 }
    }
}

impl Camera {
    /// Unit vector from the pivot to the eye.
    fn toward_eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(cp * sy, sp, cp * cy)
    }

    pub fn eye(&self) -> Vec3 {
        self.pivot + self.distance * self.toward_eye()
    }

    /// Projection times view, for a viewport `aspect` wide per unit high.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        // The clip planes hug the scene: near as far out as it allows (depth
        // precision lives there), far just past its back. The scene is the
        // model AND the grid floor under it, whose corners reach about 2.5
        // radii from the centre; hence 3.
        let near = (self.distance - CLIP_RADII * self.radius).max(self.distance * 0.01);
        let far = self.distance + CLIP_RADII * self.radius;
        let proj = Mat4::perspective_rh(FOV_Y_DEGREES.to_radians(), aspect.max(1e-3), near, far.max(near * 2.0));
        proj * Mat4::look_at_rh(self.eye(), self.pivot, Vec3::Y)
    }

    /// Centre on `centre` and back off until a sphere of `radius` round it
    /// shows whole in a viewport `aspect` wide per unit high, from the home
    /// three-quarter view.
    pub fn frame(&mut self, centre: Vec3, radius: f32, aspect: f32) {
        self.pivot = centre;
        self.radius = radius.max(1e-6);
        self.yaw = HOME_YAW;
        self.pitch = HOME_PITCH;
        self.distance = self.fit_distance(aspect);
    }

    /// The distance at which a sphere of `radius` round the pivot just fits
    /// the narrower of the two fields of view.
    fn fit_distance(&self, aspect: f32) -> f32 {
        let half_v = FOV_Y_DEGREES.to_radians() / 2.0;
        let half_h = (half_v.tan() * aspect.max(1e-3)).atan();
        FRAME_MARGIN * self.radius / half_v.min(half_h).sin()
    }

    /// Turn by a drag of (dx, dy) logical px: the surface under the pointer
    /// follows it, so dragging right swings the model's front to the right.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * ORBIT_RAD_PER_PX;
        self.pitch = (self.pitch + dy * ORBIT_RAD_PER_PX).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Slide the pivot by a drag of (dx, dy) logical px in a viewport
    /// `view_h` logical px high, so the point under the pointer stays under it.
    pub fn pan(&mut self, dx: f32, dy: f32, view_h: f32) {
        let per_px = 2.0 * self.distance * (FOV_Y_DEGREES.to_radians() / 2.0).tan() / view_h.max(1.0);
        let forward = -self.toward_eye();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(forward);
        self.pivot += (-right * dx + up * dy) * per_px;
    }

    /// Move the eye toward the pivot (`factor` < 1) or away from it.
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(self.radius * 1e-3, self.radius * 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where a world point lands in normalized device coordinates.
    fn ndc(cam: &Camera, aspect: f32, p: Vec3) -> Vec3 {
        cam.view_proj(aspect).project_point3(p)
    }

    #[test]
    fn a_framed_box_fits_the_view_at_any_aspect() {
        for aspect in [0.5, 1.0, 1.6, 3.0] {
            let mut cam = Camera::default();
            let (lo, hi) = (Vec3::new(-3.0, 0.0, -1.0), Vec3::new(5.0, 2.0, 1.0));
            cam.frame((lo + hi) / 2.0, (hi - lo).length() / 2.0, aspect);
            for i in 0..8 {
                let p = Vec3::new(
                    if i & 1 == 0 { lo.x } else { hi.x },
                    if i & 2 == 0 { lo.y } else { hi.y },
                    if i & 4 == 0 { lo.z } else { hi.z },
                );
                let q = ndc(&cam, aspect, p);
                assert!(q.x.abs() <= 1.0 && q.y.abs() <= 1.0, "aspect {aspect}: corner {p} at {q}");
                assert!((0.0..=1.0).contains(&q.z), "aspect {aspect}: corner {p} clipped in depth ({})", q.z);
            }
        }
    }

    #[test]
    fn a_pan_keeps_the_point_under_the_pointer() {
        let mut cam = Camera::default();
        cam.frame(Vec3::ZERO, 3f32.sqrt(), 1.0);
        let before = ndc(&cam, 1.0, cam.pivot);
        let pivot = cam.pivot;
        // 100 px right in a 1000 px view is 0.2 of NDC's 2-unit width.
        cam.pan(100.0, 0.0, 1000.0);
        let after = ndc(&cam, 1.0, pivot);
        assert!((after.x - before.x - 0.2).abs() < 1e-3, "moved {} in x", after.x - before.x);
        assert!((after.y - before.y).abs() < 1e-3);
    }

    #[test]
    fn dragging_right_brings_the_left_side_round() {
        let mut cam = Camera { yaw: 0.0, pitch: 0.0, ..Camera::default() };
        let left = Vec3::new(-1.0, 0.0, 0.0);
        cam.orbit(100.0, 0.0);
        // The eye has swung toward -X, the side that was on the left.
        assert!(cam.eye().dot(left) > 0.0);
    }

    #[test]
    fn pitch_stops_short_of_the_pole() {
        let mut cam = Camera::default();
        cam.orbit(0.0, 1e6);
        assert!(cam.pitch < std::f32::consts::FRAC_PI_2);
        assert!(cam.view_proj(1.0).is_finite());
    }
}
