//! cce-model — a viewer for 3D model files.
//!
//! Opens STL, OBJ, glTF/GLB and PLY (read by cce-mesh-io), frames the model
//! in a three-quarter view over a grid floor and lets you turn it. Files are
//! read and lit on a worker thread (`cce_mesh_io::load` + `light::bake`),
//! then uploaded once and drawn through cce-ui's scene pass as one prelit
//! mesh under a screen-space background gradient. The whole window is the
//! scene. The wireframe and normals overlays are built on a worker the
//! first time either is asked for, so a big model does not pay for them
//! unless they are wanted.
//!
//! `r` swaps in a path-traced view (cce-ui's tracer): whenever the camera
//! has been still for `STILL`, each frame adds a sample until the cap
//! (`trace::SAMPLES_ON_MAINS`, fewer on battery), after which nothing more
//! is staged and the GPU is left idle with the result on screen. Any
//! camera move drops straight back to the raster view.
//!
//! Mouse: drag orbits, shift+drag or middle-drag pans, ctrl+wheel and pinch
//! zoom, a two-finger scroll orbits (and coasts), as in cce-designer. A file
//! dropped on the window opens. Keys: o open · ←/→ the folder's other models
//! · 0 frame all · g grid · w wireframe · n normals · r traced · q quit.

mod camera;
mod files;
mod light;
mod overlay;
mod trace;
mod units;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cce_mesh_io::{Mesh, Unit, UpAxis};
use cce_ui::engine::{
    AppSender, Application, LogicalPosition, LogicalSize, MeshId, RtCamera, RtMaterial, RtTriangle, SceneDraw, Stage3D,
    Vertex3D, WindowSettings,
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

/// How strongly the wireframe shows over the model.
const WIRE_OPACITY: f32 = 0.55;

const HINTS: &str =
    "o open   ·   ←/→ folder   ·   0 frame   ·   g grid   ·   w wireframe   ·   n normals   ·   r traced";

/// How long the camera must rest before the traced view takes over.
const STILL: Duration = Duration::from_millis(150);

/// Where the system lists its power supplies, for the battery check.
const POWER_SUPPLIES: &str = "/sys/class/power_supply";

#[derive(Debug, Clone)]
enum Message {
    /// The model behind an `Arc`: the runner may clone a message, and a
    /// clone must not copy the vertex buffer.
    Loaded { generation: u64, path: PathBuf, result: Result<Arc<Model>, String> },
    /// The wireframe and the normals of the model of `generation`.
    Overlays { generation: u64, edges: Arc<Vec<Vertex3D>>, normals: Arc<Vec<Vertex3D>> },
    /// The traced view's scene for the model of `generation`.
    TraceScene { generation: u64, scene: Arc<TraceScene> },
    Quit,
}

/// A model read, welded, fitted and lit, ready to upload.
#[derive(Debug)]
struct Model {
    name: String,
    /// In the fitted space it is drawn in: the overlays are built from it.
    mesh: Mesh,
    parts: usize,
    /// Its box's sides in file units, upright, and what those units are.
    size: Vec3,
    unit: Unit,
    /// Kept after upload: a replacement renderer (a reconnect) starts with
    /// no meshes, and these are what go back up.
    verts: Vec<Vertex3D>,
    grid: Vec<Vertex3D>,
    grid_step: f32,
    /// The grid's height in the fitted space: where the traced ground is.
    floor_y: f32,
}

/// The tracer's triangles and materials, built on a worker.
#[derive(Debug)]
struct TraceScene {
    triangles: Vec<RtTriangle>,
    materials: Vec<RtMaterial>,
}

impl Model {
    fn read(path: &Path) -> Result<Model, String> {
        let scene = cce_mesh_io::load(path)?;
        let mut mesh = scene.merged();
        // STL is Z-up by convention; the view is Y-up. Turned here, not by
        // the reader, which reports the file as it is.
        if scene.up == UpAxis::Z {
            mesh.z_up_to_y_up();
        }
        let (lo, hi) = mesh.bounds().ok_or("the file holds no points")?;
        // Every model is drawn inside the unit sphere, so the camera frames
        // that sphere whatever the file's units.
        let (centre, radius) = mesh.fit_to_unit();
        let grid = overlay::grid(&overlay::Fit { centre, radius, lo, hi });
        Ok(Model {
            name: path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string(),
            parts: scene.parts.len(),
            size: hi - lo,
            unit: scene.unit,
            verts: light::bake(&mesh),
            mesh,
            grid: grid.lines,
            grid_step: grid.step,
            floor_y: (lo.y - centre.y) / radius,
        })
    }
}

/// The wireframe and normals of the current model, once built.
struct Overlays {
    edges: Arc<Vec<Vertex3D>>,
    normals: Arc<Vec<Vertex3D>>,
}

/// What the current renderer holds for us. A slot is created at its first
/// upload (a mesh of no vertices would be a zero-sized buffer) and updated
/// in place after, so stepping through a folder does not leak meshes.
struct Gpu {
    background: MeshId,
    model: Option<MeshId>,
    grid: Option<MeshId>,
    edges: Option<MeshId>,
    normals: Option<MeshId>,
}

/// Which of the CPU-side vertex lists still have to go up.
#[derive(Default, Clone, Copy)]
struct Pending {
    model: bool,
    grid: bool,
    edges: bool,
    normals: bool,
}

fn upload(stage: &mut dyn Stage3D, slot: &mut Option<MeshId>, verts: &[Vertex3D]) {
    if verts.is_empty() {
        return;
    }
    match *slot {
        Some(id) => stage.update_mesh(id, verts),
        None => *slot = Some(stage.create_mesh(verts)),
    }
}

enum Drag {
    Orbit,
    Pan,
}

struct ModelApp {
    sender: AppSender<Message>,
    /// Bumped by every open, so a slow load (or overlay build) the user has
    /// moved past is dropped when it arrives.
    generation: u64,
    loading: Option<PathBuf>,
    model: Option<Arc<Model>>,
    error: Option<String>,
    /// The folder's models, for ←/→, and where the open one is among them.
    files: Vec<PathBuf>,
    file_at: usize,
    overlays: Option<Overlays>,
    building_overlays: bool,
    show_grid: bool,
    show_wire: bool,
    show_normals: bool,
    /// The path-traced view: asked for, its scene once built, whether the
    /// current renderer holds it, and how far it has refined.
    traced: bool,
    trace_scene: Option<Arc<TraceScene>>,
    building_trace: bool,
    trace_uploaded: bool,
    /// A frame saying "preparing" has gone out ahead of the upload, which
    /// freezes the window while cce-ui builds the tracer's BVH (2.4 s for
    /// 5M triangles), so the freeze is explained before it happens.
    trace_announced: bool,
    samples: u32,
    sample_cap: u32,
    /// The camera and pane last staged, and when they last changed: the
    /// tracer waits for `STILL` after any change.
    view_key: [u32; 8],
    moved_at: Instant,
    /// When the current refinement began, to log how long it took.
    trace_began: Option<Instant>,
    camera: Camera,
    gpu: Option<Gpu>,
    pending: Pending,
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

    /// Read `path` on a worker. A file opened from outside the folder list
    /// (the dialog, a drop, the command line) makes its folder the list.
    fn open(&mut self, path: PathBuf, new_folder: bool) {
        if new_folder {
            (self.files, self.file_at) = files::siblings(&path);
        }
        self.generation += 1;
        let generation = self.generation;
        self.loading = Some(path.clone());
        self.error = None;
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            // A reader that panics on a malformed file must not leave the
            // window saying "Loading…" for ever: the panic becomes the error.
            let started = std::time::Instant::now();
            let result = std::panic::catch_unwind(|| Model::read(&path))
                .unwrap_or_else(|_| Err("the reader crashed on this file".to_string()))
                .map(Arc::new);
            log::info!("[model] read and lit in {:.0} ms", started.elapsed().as_secs_f64() * 1e3);
            // Fails only once the app has gone, when nobody wants the model.
            let _ = sender.send(Message::Loaded { generation, path, result });
        });
    }

    fn open_dialog(&mut self) {
        let filters: &[(&str, &[&str])] = &[
            ("3D models", cce_mesh_io::EXTENSIONS),
            ("STL", &["stl"]),
            ("OBJ", &["obj"]),
            ("glTF", &["gltf", "glb"]),
            ("PLY", &["ply"]),
        ];
        if let Some(path) = cce_ui::file_dialog::pick_file("Open model", filters) {
            self.open(path, true);
        }
    }

    /// The next (`step` 1) or previous (−1) model in the folder, wrapping.
    fn step_file(&mut self, step: i64) {
        if self.files.len() < 2 {
            return;
        }
        let n = self.files.len() as i64;
        self.file_at = (self.file_at as i64 + step).rem_euclid(n) as usize;
        self.open(self.files[self.file_at].clone(), false);
    }

    /// Build the wireframe and normals on a worker, if they are wanted and
    /// not built or being built.
    fn want_overlays(&mut self) {
        if !(self.show_wire || self.show_normals) || self.overlays.is_some() || self.building_overlays {
            return;
        }
        let Some(model) = self.model.clone() else { return };
        self.building_overlays = true;
        let (generation, sender) = (self.generation, self.sender.clone());
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let edges = Arc::new(overlay::edges(&model.mesh));
            let normals = Arc::new(overlay::normals(&model.mesh, light::CREASE_DEGREES));
            log::info!(
                "[model] {} edges and {} normals in {:.0} ms",
                edges.len() / 2,
                normals.len() / 2,
                started.elapsed().as_secs_f64() * 1e3
            );
            let _ = sender.send(Message::Overlays { generation, edges, normals });
        });
    }

    /// Build the tracer's scene on a worker, if the traced view is wanted
    /// and it is not built or being built.
    fn want_trace(&mut self) {
        if !self.traced || self.trace_scene.is_some() || self.building_trace {
            return;
        }
        let Some(model) = self.model.clone() else { return };
        self.building_trace = true;
        let (generation, sender) = (self.generation, self.sender.clone());
        std::thread::spawn(move || {
            let started = Instant::now();
            let (triangles, materials) = trace::scene(&model.mesh, model.floor_y);
            log::info!(
                "[model] trace scene: {} triangles, {} materials in {:.0} ms",
                triangles.len(),
                materials.len(),
                started.elapsed().as_secs_f64() * 1e3
            );
            let _ = sender.send(Message::TraceScene { generation, scene: Arc::new(TraceScene { triangles, materials }) });
        });
    }

    fn toggle_trace(&mut self) {
        self.traced = !self.traced;
        self.samples = 0;
        self.trace_began = None;
        // Asked once a toggle: a plug pulled mid-session takes effect at the
        // next `r` or the next model.
        self.sample_cap = if trace::on_battery(Path::new(POWER_SUPPLIES)) {
            trace::SAMPLES_ON_BATTERY
        } else {
            trace::SAMPLES_ON_MAINS
        };
        self.want_trace();
        self.scene_dirty = true;
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

    /// The HUD: the file (and its place in the folder), then what it holds.
    fn hud_lines(&self) -> Vec<String> {
        let place = if self.files.len() > 1 { format!("   ·   {} of {}", self.file_at + 1, self.files.len()) } else { String::new() };
        if let Some(path) = &self.loading {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            return vec![format!("Loading {name}…{place}")];
        }
        let Some(m) = &self.model else { return Vec::new() };
        let mut facts = vec![
            format!("{} triangles", group_thousands(m.mesh.triangles.len())),
            format!("{} points", group_thousands(m.mesh.positions.len())),
        ];
        if m.parts > 1 {
            facts.push(format!("{} parts", m.parts));
        }
        facts.push(units::dimensions(m.size, m.unit));
        if self.show_grid {
            facts.push(format!("grid {}", units::length(m.grid_step, m.unit)));
        }
        if self.traced {
            let battery = if self.sample_cap < trace::SAMPLES_ON_MAINS { " (battery)" } else { "" };
            facts.push(if self.building_trace || (self.trace_scene.is_some() && !self.trace_uploaded) {
                "traced: preparing…".into()
            } else if self.samples >= self.sample_cap {
                format!("traced, {} samples{battery}", self.samples)
            } else {
                format!("traced {} / {}{battery}", self.samples, self.sample_cap)
            });
        }
        if self.building_overlays {
            facts.push("building overlays…".into());
        } else if self.show_wire && m.mesh.triangles.len() > overlay::MAX_WIRE_TRIANGLES {
            facts.push(format!("no wireframe over {} triangles", group_thousands(overlay::MAX_WIRE_TRIANGLES)));
        }
        vec![format!("{}{place}", m.name), facts.join("   ·   ")]
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

/// The camera and pane, as bits: any change restarts the tracer's wait.
fn view_key(c: &Camera, pw: u32, ph: u32) -> [u32; 8] {
    [c.yaw.to_bits(), c.pitch.to_bits(), c.distance.to_bits(), c.pivot.x.to_bits(), c.pivot.y.to_bits(), c.pivot.z.to_bits(), pw, ph]
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
        screen_space: false,
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
            files: Vec::new(),
            file_at: 0,
            overlays: None,
            building_overlays: false,
            show_grid: true,
            show_wire: false,
            show_normals: false,
            traced: false,
            trace_scene: None,
            building_trace: false,
            trace_uploaded: false,
            trace_announced: false,
            samples: 0,
            sample_cap: trace::SAMPLES_ON_MAINS,
            view_key: [0; 8],
            moved_at: Instant::now(),
            trace_began: None,
            camera: Camera::default(),
            gpu: None,
            pending: Pending::default(),
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
            app.open(PathBuf::from(path), true);
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
                        log::info!("[model] {}: {} triangles", path.display(), model.mesh.triangles.len());
                        self.model = Some(model);
                        self.overlays = None;
                        self.building_overlays = false;
                        self.pending = Pending { model: true, grid: true, edges: false, normals: false };
                        self.trace_scene = None;
                        self.building_trace = false;
                        self.trace_uploaded = false;
                        self.trace_announced = false;
                        self.samples = 0;
                        self.frame_all();
                        self.want_overlays();
                        self.want_trace();
                    }
                    Err(e) => {
                        log::warn!("[model] {}: {e}", path.display());
                        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                        self.error = Some(format!("{name}: {e}"));
                    }
                }
                *needs_rebuild = true;
            }
            Message::TraceScene { generation, scene } => {
                if generation != self.generation {
                    return;
                }
                self.building_trace = false;
                self.trace_scene = Some(scene);
                self.trace_uploaded = false;
                self.trace_announced = false;
                self.samples = 0;
                *needs_rebuild = true;
            }
            Message::Overlays { generation, edges, normals } => {
                if generation != self.generation {
                    return;
                }
                self.building_overlays = false;
                self.overlays = Some(Overlays { edges, normals });
                self.pending.edges = true;
                self.pending.normals = true;
                self.scene_dirty = true;
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
    /// a new renderer holds no meshes, so everything goes up again.
    fn init_3d(&mut self, stage: &mut dyn Stage3D) {
        // Drawn as a `screen_space` draw: these corners are NDC, at the far
        // plane, untouched by the mvp; their z is ignored.
        let bg = |x: f32, y: f32, color: [f32; 3]| Vertex3D { position: [x, y, 0.0], color };
        let background = stage.create_mesh(&[
            bg(-1.0, -1.0, SKY_BOTTOM),
            bg(1.0, -1.0, SKY_BOTTOM),
            bg(1.0, 1.0, SKY_TOP),
            bg(-1.0, -1.0, SKY_BOTTOM),
            bg(1.0, 1.0, SKY_TOP),
            bg(-1.0, 1.0, SKY_TOP),
        ]);
        stage.set_scene_light(light::KEY.normalize().to_array());
        self.gpu = Some(Gpu { background, model: None, grid: None, edges: None, normals: None });
        let has = self.model.is_some();
        let built = self.overlays.is_some();
        self.pending = Pending { model: has, grid: has, edges: built, normals: built };
        // The tracer's scene and its accumulation died with the old renderer.
        self.trace_uploaded = false;
        self.trace_announced = false;
        self.samples = 0;
        self.scene_dirty = true;
    }

    fn stage_3d(&mut self, stage: &mut dyn Stage3D, size: LogicalSize, scale: f64) -> bool {
        let Some(gpu) = self.gpu.as_mut() else { return false };
        let pending = std::mem::take(&mut self.pending);
        if let Some(m) = &self.model {
            if pending.model {
                // On the UI thread, unavoidably: the stage is only lent here.
                // Logged, because it is the one part of a load the window waits on.
                let started = std::time::Instant::now();
                upload(stage, &mut gpu.model, &m.verts);
                log::info!("[model] uploaded {} vertices in {:.0} ms", m.verts.len(), started.elapsed().as_secs_f64() * 1e3);
            }
            if pending.grid {
                upload(stage, &mut gpu.grid, &m.grid);
            }
        }
        if let Some(o) = &self.overlays {
            if pending.edges {
                upload(stage, &mut gpu.edges, &o.edges);
            }
            if pending.normals {
                upload(stage, &mut gpu.normals, &o.normals);
            }
        }
        if pending.model || pending.grid || pending.edges || pending.normals {
            self.scene_dirty = true;
        }
        let (pw, ph) = ((size.width as f64 * scale) as u32, (size.height as f64 * scale) as u32);
        let view_proj = self.camera.view_proj(pw as f32 / ph.max(1) as f32);

        // The traced view: once the camera has rested, a sample a frame up
        // to the cap; then nothing, and the backdrop keeps the image.
        let key = view_key(&self.camera, pw, ph);
        if key != self.view_key {
            self.view_key = key;
            self.moved_at = Instant::now();
            self.samples = 0;
            self.trace_began = None;
        }
        let tracing = self.traced && self.model.is_some() && self.trace_scene.is_some();
        if tracing && self.moved_at.elapsed() >= STILL {
            if self.samples >= self.sample_cap {
                return false;
            }
            if !self.trace_uploaded {
                // The HUD is painted before this runs: return once so the
                // frame saying "preparing" is presented, and upload on the
                // next.
                if !std::mem::replace(&mut self.trace_announced, true) {
                    return true;
                }
                let scene = self.trace_scene.as_ref().unwrap();
                let started = Instant::now();
                stage.set_rt_scene(&scene.triangles, &scene.materials);
                stage.set_rt_environment(trace::environment(light::KEY.normalize().to_array()));
                stage.set_rt_background(Some(trace::BACKGROUND));
                log::info!("[model] trace scene uploaded in {:.0} ms", started.elapsed().as_secs_f64() * 1e3);
                self.trace_uploaded = true;
            }
            let began = *self.trace_began.get_or_insert_with(Instant::now);
            stage.stage_rt((0, 0, pw, ph), RtCamera { inv_mvp: view_proj.inverse().to_cols_array_2d() });
            self.samples += 1;
            // Whatever is staged next (a move's raster frame) must restage.
            self.scene_dirty = true;
            if self.samples == self.sample_cap {
                log::info!("[model] traced {} samples in {:.0} ms; idle", self.samples, began.elapsed().as_secs_f64() * 1e3);
            }
            // One frame more than samples: the HUD is painted before this
            // runs, so the frame after the last sample is the one that says
            // the cap was reached; that frame stages nothing (above).
            return true;
        }
        if !std::mem::replace(&mut self.scene_dirty, false) {
            // Still waiting out STILL: keep the frames coming to notice it.
            return tracing;
        }
        let mvp = view_proj.to_cols_array_2d();
        // One logical px, whatever the output scale (clamped by the device).
        let line_width = scale as f32;
        let lines = |mesh| SceneDraw { wireframe: true, line_width, ..draw(mesh, mvp) };
        let mut draws = vec![SceneDraw { screen_space: true, ..draw(gpu.background, mvp) }];
        if self.model.is_some() {
            let wire = self.show_wire && self.overlays.is_some();
            if let (true, Some(id)) = (self.show_grid, gpu.grid) {
                draws.push(lines(id));
            }
            if let Some(id) = gpu.model {
                let base = if wire { line_width } else { 0.0 };
                draws.push(SceneDraw { prelit: true, wire_base_width: base, ..draw(id, mvp) });
            }
            if let (true, Some(id)) = (wire, gpu.edges) {
                draws.push(SceneDraw { opacity: WIRE_OPACITY, ..lines(id) });
            }
            if let (true, true, Some(id)) = (self.show_normals, self.overlays.is_some(), gpu.normals) {
                draws.push(lines(id));
            }
        }
        stage.stage_scene((0, 0, pw, ph), draws);
        tracing
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

    fn drop_mimes(&self) -> &'static [&'static str] {
        &["text/uri-list"]
    }

    /// A file dragged in from cce-files (or anything that offers a uri
    /// list): the first one the viewer reads opens, and its folder becomes
    /// the ←/→ list.
    fn handle_drop(&mut self, _mime: &str, data: &[u8], _pos: LogicalPosition, needs_rebuild: &mut bool) {
        let paths = files::dropped_paths(data);
        match paths.iter().find(|p| files::readable(p)) {
            Some(p) => self.open(p.clone(), true),
            None => {
                let what = paths.first().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
                self.error = Some(match what {
                    Some(name) => format!("{name}: not a model this viewer reads"),
                    None => "nothing to open in that drop".into(),
                });
            }
        }
        *needs_rebuild = true;
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
            Key::Character(c) if c == "g" => self.show_grid = !self.show_grid,
            Key::Character(c) if c == "w" => {
                self.show_wire = !self.show_wire;
                self.want_overlays();
            }
            Key::Character(c) if c == "n" => {
                self.show_normals = !self.show_normals;
                self.want_overlays();
            }
            Key::Character(c) if c == "r" => self.toggle_trace(),
            Key::Character(c) if c == "q" => return Some(Message::Quit),
            Key::Named(NamedKey::ArrowLeft) => self.step_file(-1),
            Key::Named(NamedKey::ArrowRight) => self.step_file(1),
            _ => return None,
        }
        self.scene_dirty = true;
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
            centre(&mut pc, "Press o to open a model (STL, OBJ, glTF or PLY), or drop one here");
        }

        let lines = self.hud_lines();
        if !lines.is_empty() {
            const LINE: f32 = 18.0;
            let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
            let w = 2.0 * text_in + longest as f32 * 6.6;
            let h = 8.0 + LINE * lines.len() as f32;
            pc.quad(Rect { x: inset, y: inset, width: w, height: h }, [0.0, 0.0, 0.0, 0.45]);
            for (i, line) in lines.into_iter().enumerate() {
                let colour = if i == 0 { [235, 235, 238] } else { [190, 190, 198] };
                pc.text(line, inset + text_in, inset + 5.0 + LINE * i as f32, 12.0, colour);
            }
        }
        if self.model.is_some() {
            pc.text(HINTS, inset, size.height - inset - 14.0, 11.0, [120, 122, 132]);
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
}
