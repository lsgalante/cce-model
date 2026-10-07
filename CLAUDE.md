# cce-model

A viewer for 3D model files. Read the workspace guide
(`../cce-compositor/WORKSPACE.md`) first; this file covers only what is
particular to this crate. The plan it is built to — formats, milestones,
what each one must pass — is the design doc "cce-model: a general-purpose
3D viewer" (claude.ai/code/artifact/1ebec744-8990-4884-ad92-73c72e0ea265).
Milestones 1 and 2 are here: STL, OBJ, glTF/GLB and PLY, read by the
shared `cce-mesh-io` crate, on the existing raster stage.

## Shape

- `src/main.rs` — the `Application`. Portable hooks only: `create`,
  `init_3d` / `stage_3d` (never the native `renderer_init` /
  `stage_renderer`). A file is read on a worker thread and comes back through
  the `AppSender` as an `Arc<Model>` (the runner clones messages); a
  generation drops a load the user has moved past, and a reader's panic is
  caught there and shown as an error. The model's baked vertices stay on
  the CPU after upload, because `init_3d` runs again for a replacement
  renderer after a reconnect and every mesh must go back up (360 MB for a
  5M-triangle model; slimming it is open).
- Reading is `cce_mesh_io::load` (see that crate's CLAUDE.md): a `Scene` of
  parts in the file's own coordinates plus its up axis. This app merges the
  parts, turns a Z-up scene upright, fits it to the unit sphere and bakes.
- `src/light.rs` — the light bake: crease-aware corner normals (from
  cce-mesh-io) against a fixed key/fill/ambient rig, into `Vertex3D`s.
- `src/camera.rs` — orbit camera: yaw, pitch, distance about a pivot.

## Things that are not obvious

- **Smooth shading is baked.** cce-ui's `Vertex3D` is position + colour, and
  its own shading is flat (screen-space derivatives). A `prelit` draw shows
  the vertex colours as they are, so `light::bake` lights every corner from
  its normal against a fixed studio rig (key, fill, sky/ground ambient). The
  rig is fixed in world space, so the bake never changes as the camera moves.
  Faces meeting past `CREASE_DEGREES` keep their own normals.
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
`0` key. The window's display is the shadow's
`WAYLAND_DISPLAY` from `cce-shadow env`, which changes when the instance
restarts — match it when picking processes to stop. The log carries
`read and lit in N ms` (worker) and `uploaded N vertices in N ms` (the UI
thread's only share of a load). Real test models: the slicers' resource STLs under
`~/.local/share/Steam/steamapps/compatdata/*/pfx/drive_c/Program Files/`
(ChiTuBox's `high_precision_sphere.stl`, Bambu Studio's calibration towers).
