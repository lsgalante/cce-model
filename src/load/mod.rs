//! Reading model files into a [`Mesh`]. One reader per format; each returns
//! the mesh as the file has it, and [`load`] welds it.
//!
//! This module is written to move out whole: milestone 2 of the viewer's
//! design doc lifts it into a shared `cce-mesh-io` crate, so cce-designer
//! can import what it already exports. Nothing in it knows about the app.

mod obj;
mod stl;

use std::path::Path;

use crate::mesh::Mesh;

/// File extensions the viewer opens, lowercase.
pub const EXTENSIONS: &[&str] = &["stl", "obj"];

/// Read `path` by its extension and weld the result.
pub fn load(path: &Path) -> Result<Mesh, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut mesh = match ext.as_str() {
        "stl" => stl::read(&bytes)?,
        "obj" => obj::read(&bytes, path.parent())?,
        "" => return Err("no file extension, so no way to tell the format".into()),
        other => return Err(format!("cannot read .{other} files (yet)")),
    };
    mesh.weld();
    if mesh.triangles.is_empty() {
        return Err("the file holds no triangles".into());
    }
    Ok(mesh)
}
