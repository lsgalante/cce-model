# cce-model

A viewer for 3D model files. Read the workspace guide
(`../cce-compositor/WORKSPACE.md`) first; this file covers only what is
particular to this crate. The plan it is built to — formats, milestones,
what each one must pass — is the design doc "cce-model: a general-purpose
3D viewer" (claude.ai/code/artifact/1ebec744-8990-4884-ad92-73c72e0ea265).
Milestones 1–5 are here: STL, OBJ, glTF/GLB and PLY read by the shared
`cce-mesh-io` crate, a grid floor, wireframe and normals overlays, sizes in
the file's units, ←/→ through the folder, drag-and-drop, a path-traced
view (`r`), and the model drawn through cce-ui's lit pipeline (per-fragment
light, metallic-roughness, base-colour textures).

## Shape

- `src/main.rs` — the `Application`. Portable hooks only: `create`,
  `init_3d` / `stage_3d` (never the native `renderer_init` /
  `stage_renderer`). A file is read on a worker thread and comes back through
  the `AppSender` as an `Arc<Model>` (the runner clones messages); a
  generation drops a load the user has moved past, and a reader's panic is
  caught there and shown as an error. The model's baked vertices stay on
  the CPU after upload, because `init_3d` runs again for a replacement
  renderer after a reconnect and every mesh must go back up (660 MB of lit
  vertices for a 5M-triangle model; slimming it is open). Textures go up as
  mipmapped image ids when a model arrives, are freed when it is replaced,
  and are uploaded again for a replacement renderer (`seen_renderer`).
- Reading is `cce_mesh_io::load` (see that crate's CLAUDE.md): a `Scene` of
  parts in the file's own coordinates plus its up axis. This app merges the
  parts, turns a Z-up scene upright, fits it to the unit sphere and bakes.
- `src/light.rs` — the lit meshes and their light: `lit_parts` turns the
  mesh into one `LitVertex` list per material (a draw binds one texture),
  with the file's normals or crease-aware ones; `rig()` is the studio
  light (key, fill, sky/ground) the lit pipeline shades by.
- `src/camera.rs` — orbit camera: yaw, pitch, distance about a pivot.
- `src/overlay.rs` — the line meshes: grid floor (laid out in FILE units on
  1-2-5 steps through the file's origin, then fitted), edges, normals.
  Edges and normals are built on a worker the first time `w` or `n` asks
  (`Message::Overlays`, tied to the load's generation). No wireframe past
  `MAX_WIRE_TRIANGLES` (2M): it paints the model solid and costs 360 MB;
  normals are thinned to `MAX_NORMALS` and drawn as long as their spacing.
- `src/trace.rs` — the traced view's scene (a material per distinct
  triangle colour, quantized; a wide ground plane at the grid's height), its
  grey sky with the sun on the raster key light, the sample caps (256 on
  mains, 32 on battery) and the `/sys/class/power_supply` battery check.
- `src/units.rs` — sizes in mm/m only when the format says what its unit
  is (`cce_mesh_io::Unit`); OBJ and PLY sizes are bare numbers.
- `src/files.rs` — the folder's models for ←/→ (any file opened from
  outside the list makes its folder the list) and `text/uri-list` drops.

## Things that are not obvious

- **The model is drawn lit, not baked** (since milestone 5): `LitDraw`s
  staged after `stage_scene`, each `before` the first wire draw so the
  background and grid go under and the overlays over. Until then the light
  was baked into `Vertex3D` colours on the CPU, which could show neither a
  texture nor a highlight that moves. `Stage3D::lit()` is `None` on a
  renderer without the pipeline; the app then says it cannot draw.
- **Every model is moved into the unit sphere** (`Mesh::fit_to_unit`)
  before it is baked, so the camera frames every file alike. (Until
  cce-ui 38bcad1 this was also a workaround: any vertex near z = 9.99 was
  drawn as the background quad. The background is a `screen_space` draw now.)
- **A Z-up scene (STL) is turned upright here, not by the reader.** So a
  cce-designer STL export, which is its Y-up world written as is, shows on
  its side — as it would in a slicer.
- **A staged scene persists** in the backdrop until the next one, so
  `stage_3d` stages only when `scene_dirty` (camera, resize, upload); a HUD
  change repaints the 2D pass alone.
- **Every GPU slot is created once and updated in place** (`upload`), so
  stepping through a folder does not leak meshes; `init_3d` clears the
  slots and marks every CPU-side list pending for a replacement renderer.
- **The clip planes keep 3 radii** around the pivot, for the grid's
  corners, not just the model's 1.
- **The traced view is a state machine in `stage_3d`.** Any change of
  camera or pane (`view_key`) resets the samples and the wait; only after
  `STILL` (150 ms) does a frame stage `stage_rt` instead of the raster
  scene, one sample a frame; at the cap it stages nothing and stops asking
  for frames, so the GPU and the process go idle (0 CPU ticks measured)
  with the traced image left in the backdrop. One frame past each edge is
  deliberate: the HUD paints BEFORE `stage_3d`, so the cap and the
  "preparing" notice each need a frame of their own to be seen.
- **The tracer's BVH is built on the worker**, with the rest of the scene
  (`PreparedRtScene::new` in `want_trace`, cce-ui ≥ e5fbe9a); `stage_3d`
  only uploads it (`set_rt_scene_prepared`). Whether to build one is the
  stage's `rt_needs_bvh()`, asked in `init_3d`: the hardware ray-query tier
  builds its own structure on the GPU and skips it. Measured 2026-10-07,
  torus5m.stl (5,001,600 triangles), scale-2 shadow: Intel iGPU (compute
  tier) worker 2032 ms, UI thread 61 ms (it was 2141 ms on the UI thread);
  RTX 4080 (ray-query, `CCE_VK_DEVICE=discrete` plus the live `DISPLAY=:0`
  in the shadow) worker 321 ms, upload with the BLAS build 146 ms. The
  prepared scene stays in memory for a reconnect's re-upload (~300 MB at
  5M with its BVH, on top of the baked vertices).
- **The tracer has no smooth normals**: a coarse sphere shows its facets
  when traced. Milestone 5's territory (cce-ui's materials and vertices).
- No root plate, on purpose (`style-audit: opt-out` in `display_list`): the
  scene is the window's content, and a root plate would frost over it.

## Verify

Headless, in a scale-2 shadow, always through `cce-shadow run`:

```sh
cce-shadow start --new --scale 2          # note the agent-N it prints
CCE_SHADOW_INSTANCE=agent-N cce-shadow ctl idle timeouts 0 0
CCE_SHADOW_INSTANCE=agent-N cce-shadow run env CCE_FONTS_DIR=$HOME/Dropbox/Fonts \
    /home/lsgalante/projects/cce/target/release/cce-model /path/to/model.stl &
```

then `ctl windows` for the id and `shot-window <id>`. The window is
1000×750 logical, larger than the 640×360 logical headless output at scale
2, so use `shot-window`, not `shot`, and keep pointer targets inside
640×360. `pointer-press` / `pointer-move-by` / `pointer-release` drive an
orbit, `pointer-pinch 1.6` a zoom, `pointer-scroll 0 -12 finger` then
`pointer-scroll finger-stop` a scroll orbit and its coast, `keypress 11` the
`0` key, 17/49/34/19 `w`/`n`/`g`/`r`, 105/106 ←/→. A traced run logs
`traced N samples in M ms; idle` at its cap. A new viewer window can map
WITHOUT focus (keys then reach nothing, or a stale window): `ctl
focus-window cce-model` after each launch, and close windows by pid
(checking `/proc/<pid>/environ` for the shadow's display) — one launched by
cce-files has argv0 `cce-model`, so a `release/cce-model` match misses it.
cce-files in a shadow finds the real desktop entries with
`XDG_DATA_HOME=$HOME/.local/share` and a tree build via a PATH symlink.
Drops are untested end to end: cce-ui has no drag SOURCE, so no cce app
can drag a file out. The window's display is the shadow's
`WAYLAND_DISPLAY` from `cce-shadow env`, which changes when the instance
restarts — match it when picking processes to stop. The log carries
`read and lit in N ms` (worker) and `uploaded N vertices in N ms` (the UI
thread's only share of a load). Real test models: the slicers' resource STLs under
`~/.local/share/Steam/steamapps/compatdata/*/pfx/drive_c/Program Files/`
(ChiTuBox's `high_precision_sphere.stl`, Bambu Studio's calibration towers).
