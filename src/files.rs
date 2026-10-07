//! Which files the viewer can step through, and what a drop hands it.

use std::path::{Path, PathBuf};

/// True when the extension is one cce-mesh-io reads.
pub fn readable(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| cce_mesh_io::EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The models in `path`'s folder, by name (case aside), and where `path` is
/// among them — what ←/→ step through. A folder that cannot be read gives
/// the file alone.
pub fn siblings(path: &Path) -> (Vec<PathBuf>, usize) {
    let mut files: Vec<PathBuf> = path
        .parent()
        .and_then(|dir| std::fs::read_dir(if dir.as_os_str().is_empty() { Path::new(".") } else { dir }).ok())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && readable(p))
        .collect();
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    let at = files.iter().position(|p| p.file_name() == path.file_name());
    match at {
        Some(i) => (files, i),
        None => (vec![path.to_path_buf()], 0),
    }
}

/// The local paths in a `text/uri-list` drop (RFC 2483: one URI a line,
/// `#` lines are comments), percent-decoded. Anything not `file:` is left
/// out: the viewer opens files, not URLs.
pub fn dropped_paths(data: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(data)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        // file://host/path: only the local host (empty, or localhost).
        .filter_map(|rest| rest.strip_prefix("localhost").or(Some(rest)).filter(|r| r.starts_with('/')))
        .map(|p| PathBuf::from(percent_decode(p)))
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_list_gives_its_local_files() {
        let list = b"# from cce-files\r\nfile:///home/me/My%20Parts/bracket.stl\r\nhttps://example.com/x.stl\r\nfile://localhost/tmp/a.glb\r\nfile://elsewhere/tmp/b.glb\r\n";
        assert_eq!(dropped_paths(list), vec![PathBuf::from("/home/me/My Parts/bracket.stl"), PathBuf::from("/tmp/a.glb")]);
    }

    #[test]
    fn siblings_are_the_models_in_the_folder_in_name_order() {
        let dir = std::env::temp_dir().join(format!("cce-model-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["b.STL", "a.obj", "notes.txt", "c.glb"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let (files, at) = siblings(&dir.join("b.STL"));
        std::fs::remove_dir_all(&dir).ok();
        let names: Vec<_> = files.iter().map(|p| p.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["a.obj", "b.STL", "c.glb"]);
        assert_eq!(at, 1);
    }
}
