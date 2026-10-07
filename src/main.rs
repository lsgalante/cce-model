//! cce-model — a viewer for 3D model files.
//!
//! Opens STL and OBJ (milestone 1 of the design doc; glTF and PLY follow),
//! frames the model in a three-quarter view and lets you turn it. Files are
//! read and lit on a worker thread (`load` + `mesh::bake`), then uploaded
//! once and drawn through cce-ui's scene pass as one prelit mesh under a
//! screen-space background gradient. The whole window is the scene.
//!
//! Mouse: drag orbits, shift+drag or middle-drag pans, ctrl+wheel and pinch
//! zoom, a two-finger scroll orbits (and coasts), as in cce-designer.
//! Keys: o open · 0 frame all · q quit.

mod camera;
mod load;
mod mesh;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cce_ui::engine::{
    AppSender, Application, LogicalPosition, LogicalSize, MeshId, SceneDraw, Stage3D, Vertex3D, WindowSettings,
};
use cce_ui::scene::layout::Rect;
use cce_ui::scene::paint::{DisplayList, PaintCtx};
use cce_ui::widget::scroll_motion::{current_scroll_phase, Bounds, ScrollMotion, ScrollPhase};
use cce_ui::widget::{ElementState, Key, KeyEvent, MouseButton, MouseScrollDelta, NamedKey};
use glam::Vec3;

use camera::Camera;

/// Wheel notches in logical px of orbit, and of log-zoom.
const ORBIT_PX_PER_LINE: f32 = 10.0;
const ZOOM_PER_LINE: f32 = 0.15;
const ZOOM_PER_PX: f32 = 0.005;

/// The background gradient, linear RGB, top and bottom.
const SKY_TOP: [f32; 3] = [0.105, 0.11, 0.125];
const SKY_BOTTOM: [f32; 3] = [0.03, 0.03, 0.035];

#[derive(Debug, Clone)]
enum Message {
    /// The model behind an `Arc`: the runner may clone a message, and a
    /// clone must not copy the vertex buffer.
    Loaded { generation: u64, path: PathBuf, result: Result<Arc<Model>, String> },
    Quit,
}

/// A model read, welded and lit, ready to upload.
#[derive(Debug)]
struct Model {
    name: String,
    triangles: usize,
    /// Kept after upload: a replacement renderer (a reconnect) starts with
    /// no meshes, and this is what goes back up.
    verts: Vec<Vertex3D>,
}

impl Model {
    fn read(path: &Path) -> Result<Model, String> {
        let mut mesh = load::load(path)?;
        // Every model is drawn inside the unit sphere (see `fit_to_unit`),
        // so the camera frames that sphere whatever the file's units.
        mesh.fit_to_unit();
        Ok(Model {
            name: path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string(),
            triangles: mesh.triangles.len(),
            verts: mesh.bake(),
        })
    }
}

/// What the current renderer holds for us.
struct Gpu {
    background: MeshId,
    /// Created at the first upload: a mesh of no vertices would be a
    /// zero-sized buffer.
    model: Option<MeshId>,
}

enum Drag {
    Orbit,
    Pan,
}

struct ModelApp {
    sender: AppSender<Message>,
    /// Bumped by every open, so a slow load the user has moved past is
    /// dropped when it arrives.
    generation: u64,
    loading: Option<PathBuf>,
    model: Option<Arc<Model>>,
    error: Option<String>,
    camera: Camera,
    gpu: Option<Gpu>,
    /// The model's vertices are not on the GPU yet.
    upload_pending: bool,
    /// The view changed since the scene was last staged. A staged scene
    /// stays in the backdrop until the next one, so a frame that only
    /// changes the HUD does not redraw the model.
    scene_dirty: bool,
    /// Two-finger scroll orbit, in logical px: a finger tracks 1:1, the
    /// lift coasts, a notch glides. Only how far it moved matters.
    orbit_motion: ScrollMotion,
    pointer: (f32, f32),
    drag: Option<Drag>,
    ctrl: bool,
    shift: bool,
    win: (f32, f32),
    scale: f64,
}

impl ModelApp {
    fn aspect(&self) -> f32 {
        self.win.0 / self.win.1.max(1.0)
    }

    fn open(&mut self, path: PathBuf) {
        self.generation += 1;
        let generation = self.generation;
        self.loading = Some(path.clone());
        self.error = None;
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = Model::read(&path).map(Arc::new);
            // Fails only once the app has gone, when nobody wants the model.
            let _ = sender.send(Message::Loaded { generation, path, result });
        });
    }

    fn open_dialog(&mut self) {
        let filters: &[(&str, &[&str])] = &[("3D models", load::EXTENSIONS), ("STL", &["stl"]), ("OBJ", &["obj"])];
        if let Some(path) = cce_ui::file_dialog::pick_file("Open model", filters) {
            self.open(path);
        }
    }

    fn frame_all(&mut self) {
        if self.model.is_some() {
            self.camera.frame(Vec3::ZERO, 1.0, self.aspect());
            self.orbit_motion = ScrollMotion::new();
            self.scene_dirty = true;
        }
    }

    /// Orbit by how far the scroll motion moved since `before`.
    fn orbit_since(&mut self, before: (f32, f32)) -> bool {
        let (dx, dy) = (self.orbit_motion.x.pos() - before.0, self.orbit_motion.y.pos() - before.1);
        if dx == 0.0 && dy == 0.0 {
            return false;
        }
        self.camera.orbit(dx, dy);
        self.scene_dirty = true;
        true
    }

    fn orbit_pos(&self) -> (f32, f32) {
        (self.orbit_motion.x.pos(), self.orbit_motion.y.pos())
    }

    fn hud_line(&self) -> Option<String> {
        if let Some(path) = &self.loading {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            return Some(format!("Loading {name}…"));
        }
        let m = self.model.as_ref()?;
        Some(format!("{}   ·   {} triangles", m.name, group_thousands(m.triangles)))
    }
}

/// 1234567 → "1,234,567".
fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A scene draw of `mesh` with every option at its plain default.
fn draw(mesh: MeshId, mvp: [[f32; 4]; 4]) -> SceneDraw {
    SceneDraw {
        mesh,
        mvp,
        wireframe: false,
        wire_tint: [0.0; 4],
        opacity: 1.0,
        line_width: 1.0,
        wire_base_width: 0.0,
        prelit: false,
        see_through: false,
        instances: None,
    }
}

impl Application for ModelApp {
    type Message = Message;

    fn create(sender: AppSender<Message>) -> Self {
        let mut app = Self {
            sender,
            generation: 0,
            loading: None,
            model: None,
            error: None,
            camera: Camera::default(),
            gpu: None,
            upload_pending: false,
            scene_dirty: true,
            orbit_motion: ScrollMotion::new(),
            pointer: (0.0, 0.0),
            drag: None,
            ctrl: false,
            shift: false,
            win: (1000.0, 750.0),
            scale: 1.0,
        };
        if let Some(path) = std::env::args_os().nth(1) {
            app.open(PathBuf::from(path));
        }
        app
    }

    fn settings(&self) -> WindowSettings {
        let title = match &self.model {
            Some(m) => format!("{} — Model", m.name),
            None => "Model".to_string(),
        };
        WindowSettings { title, app_id: "cce-model".into(), width: 1000, height: 750, fullscreen: false, min_size: Some((320, 240)) }
    }

    fn update(&mut self, msg: Message, needs_rebuild: &mut bool, exit: &mut bool) {
        match msg {
            Message::Quit => *exit = true,
            Message::Loaded { generation, path, result } => {
                if generation != self.generation {
                    return;
                }
                self.loading = None;
                match result {
                    Ok(model) => {
                        log::info!("[model] {}: {} triangles", path.display(), model.triangles);
                        self.model = Some(model);
                        self.upload_pending = true;
                        self.frame_all();
                    }
                    Err(e) => {
                        log::warn!("[model] {}: {e}", path.display());
                        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                        self.error = Some(format!("{name}: {e}"));
                    }
                }
                *needs_rebuild = true;
            }
        }
    }

    fn tick(&mut self, dt: f32, needs_rebuild: &mut bool) {
        if self.orbit_motion.is_animating() {
            let before = self.orbit_pos();
            self.orbit_motion.tick(dt, Bounds::UNBOUNDED, Bounds::UNBOUNDED);
            if self.orbit_since(before) || self.orbit_motion.is_animating() {
                *needs_rebuild = true;
            }
        }
    }

    fn load_system_fonts(&self) -> bool {
        true
    }

    fn display_list_text(&self) -> bool {
        true
    }

    /// Once per renderer, the first and any replacement after a reconnect:
    /// a new renderer holds no meshes, so the model goes up again.
    fn init_3d(&mut self, stage: &mut dyn Stage3D) {
        let bg = |x: f32, y: f32, color: [f32; 3]| Vertex3D { position: [x, y, 9.99], color };
        // z = 9.99 is the scene pass's screen-space sentinel: these corners
        // are NDC, drawn behind everything, untouched by the mvp.
        let background = stage.create_mesh(&[
            bg(-1.0, -1.0, SKY_BOTTOM),
            bg(1.0, -1.0, SKY_BOTTOM),
            bg(1.0, 1.0, SKY_TOP),
            bg(-1.0, -1.0, SKY_BOTTOM),
            bg(1.0, 1.0, SKY_TOP),
            bg(-1.0, 1.0, SKY_TOP),
        ]);
        stage.set_scene_light(mesh::KEY.normalize().to_array());
        self.gpu = Some(Gpu { background, model: None });
        self.upload_pending = self.model.is_some();
        self.scene_dirty = true;
    }

    fn stage_3d(&mut self, stage: &mut dyn Stage3D, size: LogicalSize, scale: f64) -> bool {
        let Some(gpu) = self.gpu.as_mut() else { return false };
        if self.upload_pending {
            if let Some(m) = &self.model {
                match gpu.model {
                    Some(id) => stage.update_mesh(id, &m.verts),
                    None => gpu.model = Some(stage.create_mesh(&m.verts)),
                }
            }
            self.upload_pending = false;
            self.scene_dirty = true;
        }
        if !std::mem::replace(&mut self.scene_dirty, false) {
            return false;
        }
        let (pw, ph) = ((size.width as f64 * scale) as u32, (size.height as f64 * scale) as u32);
        let mvp = self.camera.view_proj(pw as f32 / ph.max(1) as f32).to_cols_array_2d();
        let mut draws = vec![draw(gpu.background, mvp)];
        if let (Some(id), Some(_)) = (gpu.model, &self.model) {
            draws.push(SceneDraw { prelit: true, ..draw(id, mvp) });
        }
        stage.stage_scene((0, 0, pw, ph), draws);
        false
    }

    fn handle_resize(&mut self, width: f32, height: f32, scale: f64) {
        self.win = (width, height);
        self.scale = scale;
        self.scene_dirty = true;
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        let (x, y) = (pos.x as f32, pos.y as f32);
        let (dx, dy) = (x - self.pointer.0, y - self.pointer.1);
        self.pointer = (x, y);
        match self.drag {
            Some(Drag::Orbit) => self.camera.orbit(dx, dy),
            Some(Drag::Pan) => self.camera.pan(dx, dy, self.win.1),
            None => return,
        }
        self.scene_dirty = true;
        *needs_rebuild = true;
    }

    fn handle_mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
        pos: LogicalPosition,
        _needs_rebuild: &mut bool,
    ) -> Option<Message> {
        self.pointer = (pos.x as f32, pos.y as f32);
        if state == ElementState::Released {
            self.drag = None;
            return None;
        }
        self.drag = match button {
            MouseButton::Left if self.shift => Some(Drag::Pan),
            MouseButton::Left => Some(Drag::Orbit),
            MouseButton::Middle => Some(Drag::Pan),
            _ => None,
        };
        if self.drag.is_some() {
            // A grab stops a coast, as a hand on a spinning turntable would.
            self.orbit_motion = ScrollMotion::new();
        }
        None
    }

    fn handle_mouse_wheel(&mut self, delta: &MouseScrollDelta, _pos: LogicalPosition, needs_rebuild: &mut bool) {
        let scale = self.scale.max(0.001) as f32;
        let discrete = matches!(delta, MouseScrollDelta::LineDelta(..));
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (*x * ORBIT_PX_PER_LINE, *y * ORBIT_PX_PER_LINE),
            MouseScrollDelta::PixelDelta(p) => (p.x as f32 / scale, p.y as f32 / scale),
        };
        if self.ctrl {
            let log_zoom = if discrete { -dy / ORBIT_PX_PER_LINE * ZOOM_PER_LINE } else { -dy * ZOOM_PER_PX };
            self.camera.zoom(log_zoom.exp());
            self.scene_dirty = true;
            *needs_rebuild = true;
            return;
        }
        let phase = if discrete { ScrollPhase::Wheel } else { current_scroll_phase() };
        let before = self.orbit_pos();
        self.orbit_motion.apply_phase(phase, dx, dy, discrete, Bounds::UNBOUNDED, Bounds::UNBOUNDED);
        if self.orbit_since(before) || self.orbit_motion.is_animating() {
            *needs_rebuild = true;
        }
    }

    fn handle_pinch(&mut self, factor: f32, _pos: LogicalPosition, needs_rebuild: &mut bool) -> bool {
        if factor > 0.0 && factor != 1.0 {
            self.camera.zoom(1.0 / factor);
            self.scene_dirty = true;
            *needs_rebuild = true;
        }
        true
    }

    fn handle_key_input(&mut self, event: &KeyEvent, needs_rebuild: &mut bool) -> Option<Message> {
        // Wheel and button events carry no modifiers, so track them from
        // the key stream (ctrl+wheel zoom, shift+drag pan).
        match &event.logical_key {
            Key::Named(NamedKey::Control) => self.ctrl = event.state == ElementState::Pressed,
            Key::Named(NamedKey::Shift) => self.shift = event.state == ElementState::Pressed,
            _ => {
                self.ctrl = event.ctrl;
                self.shift = event.shift;
            }
        }
        if event.state != ElementState::Pressed {
            return None;
        }
        match &event.logical_key {
            Key::Character(c) if c == "o" => self.open_dialog(),
            Key::Character(c) if c == "0" => self.frame_all(),
            Key::Character(c) if c == "q" => return Some(Message::Quit),
            _ => return None,
        }
        *needs_rebuild = true;
        None
    }

    // style-audit: opt-out the 3D scene is the window's content, full-bleed under the HUD; a root plate would frost over it.
    fn display_list(&mut self, size: LogicalSize, scale: f64) -> Option<DisplayList> {
        self.win = (size.width, size.height);
        self.scale = scale;
        let mut pc = PaintCtx::new();
        let inset = cce_ui::layout::root_plate_inset();
        let text_in = cce_ui::layout::CONTROL_TEXT_INSET;

        let centre = |pc: &mut PaintCtx, msg: &str| {
            let w = msg.chars().count() as f32 * 7.4;
            let x = ((size.width - w) / 2.0).max(inset);
            pc.text(msg.to_string(), x, size.height / 2.0 - 8.0, 14.0, [190, 190, 196]);
        };
        if let Some(e) = &self.error {
            centre(&mut pc, e);
        } else if self.model.is_none() && self.loading.is_none() {
            centre(&mut pc, "Press o to open a model (STL or OBJ)");
        }

        if let Some(hud) = self.hud_line() {
            let w = 2.0 * text_in + hud.chars().count() as f32 * 6.6;
            pc.quad(Rect { x: inset, y: inset, width: w, height: 24.0 }, [0.0, 0.0, 0.0, 0.45]);
            pc.text(hud, inset + text_in, inset + 5.0, 12.0, [230, 230, 230]);
        }
        Some(pc.finish())
    }

    fn clear_color(&self) -> [f32; 4] {
        [SKY_BOTTOM[0], SKY_BOTTOM[1], SKY_BOTTOM[2], 1.0]
    }
}

fn main() {
    env_logger::init();
    cce_ui::engine::run::<ModelApp>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(group_thousands(7), "7");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(1234567), "1,234,567");
    }

    #[test]
    fn a_soup_cube_bakes_to_twelve_lit_triangles() {
        let mut m = mesh::soup_cube();
        m.weld();
        let verts = m.bake();
        assert_eq!(verts.len(), 36);
        // Six faces, each one flat colour from its own normal: six distinct shades.
        let mut shades: Vec<u32> = verts.iter().map(|v| (v.color[0] * 1e4) as u32).collect();
        shades.sort();
        shades.dedup();
        assert_eq!(shades.len(), 6, "{shades:?}");
    }
}
