# cce-model

A viewer for 3D model files. Read the workspace guide
(`../cce-compositor/WORKSPACE.md`) first; this file covers only what is
particular to this crate. The plan it is built to — formats, milestones,
what each one must pass — is the design doc "cce-model: a general-purpose
3D viewer" (claude.ai/code/artifact/1ebec744-8990-4884-ad92-73c72e0ea265).
Milestone 1 (STL and OBJ on the existing raster stage) is what is here.

## Shape

- `src/main.rs` — the `Application`. Portable hooks only: `create`,
  `init_3d` / `stage_3d` (never the native `renderer_init` /
  `stage_renderer`). A file is read on a worker thread and comes back through
  the `AppSender` as an `Arc<Model>` (the runner clones messages); a
  generation drops a load the user has moved past. The model's baked
  vertices stay on the CPU after upload, because `init_3d` runs again for a
  replacement renderer after a reconnect and every mesh must go back up.
- `src/load/` — one reader per format (`stl.rs`, `obj.rs`), each returning
  a `Mesh` as the file has it; `load()` welds. Nothing here knows about the
  app: milestone 2 lifts it whole into a shared `cce-mesh-io` crate.
- `src/mesh.rs` — weld, crease-aware corner normals, and the light bake.
- `src/camera.rs` — orbit camera: yaw, pitch, distance about a pivot.

## Things that are not obvious

- **Smooth shading is baked.** cce-ui's `Vertex3D` is position + colour, and
  its own shading is flat (screen-space derivatives). A `prelit` draw shows
  the vertex colours as they are, so `mesh::bake` lights every corner from
  its normal against a fixed studio rig (key, fill, sky/ground ambient). The
  rig is fixed in world space, so the bake never changes as the camera moves.
  Faces meeting past `CREASE_DEGREES` keep their own normals.
- **Every model is moved into the unit sphere** (`Mesh::fit_to_unit`) before
  it is baked. Partly for the camera, but mainly because cce-ui's
  `scene3d.wgsl` treats ANY vertex with |z − 9.99| < 0.01 as a corner of the
  screen-space background quad, in whatever mesh it appears. A 25 mm sphere
  spanning z = 9.99 had a ring of its points flung across the window as
  spikes. Do not upload geometry in file units.
- **STL is turned Z-up → Y-up on load** ((x, y, z) → (x, z, −y), a rotation,
  so winding is kept). OBJ is taken as Y-up, as it nearly always is.
- **Binary vs ASCII STL is decided by size** (84 + 50·n bytes), not by the
  word `solid`, which many binary headers begin with.
- **A staged scene persists** in the backdrop until the next one, so
  `stage_3d` stages only when `scene_dirty` (camera, resize, upload); a HUD
  change repaints the 2D pass alone.
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
`0` key. Real test models: the slicers' resource STLs under
`~/.local/share/Steam/steamapps/compatdata/*/pfx/drive_c/Program Files/`
(ChiTuBox's `high_precision_sphere.stl`, Bambu Studio's calibration towers).
