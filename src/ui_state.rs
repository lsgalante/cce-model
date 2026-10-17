//! Where the viewer was, so it comes back there: the model that was open and
//! the camera on it. Saved when a model opens and when the window exits
//! (`on_exit`, which `ccectl restart-app` runs as any close does), taken back
//! when one launches — after a restart, at login, or when reopened.
//!
//! **A restart replays the same command line**, so a viewer launched on
//! `a.stl` that has since stepped to `b.stl` would come back on `a.stl`. The
//! file therefore also records the model the command line named
//! (`launched`): the same command line again is this viewer reopening, and
//! the saved model and view win; a command line naming another model is a new
//! request, and it opens framed as any open does. A command line naming
//! nothing (the launcher, login) reopens what was open last. A saved model
//! that is gone is skipped. Restoring only reopens: nothing here writes to a model file.
//!
//! `cce_state_dir()/model/state.json`, written whole to a temporary file and
//! renamed over, so a crash mid-write leaves the last one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// The orbit camera, as `Camera` holds it (its `radius` is the fitted
/// model's, always 1, so it is not kept).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub pivot: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Saved {
    /// The model that was open.
    file: PathBuf,
    view: Option<View>,
    /// The model the command line named when this was saved, if any.
    launched: Option<PathBuf>,
}

/// What a launch opens, and the view to put it in (`None`: framed).
#[derive(Debug, Default, PartialEq)]
pub struct Restore {
    pub file: Option<PathBuf>,
    pub view: Option<View>,
}

/// The model this run's command line named.
static LAUNCHED: OnceLock<Option<PathBuf>> = OnceLock::new();

fn path() -> PathBuf {
    cce_ui::config::cce_state_dir().join("model").join("state.json")
}

/// What to open: the saved model and view when this command line names
/// nothing or the model they were saved under, else the model it names.
pub fn initial(argv: Option<PathBuf>) -> Restore {
    // Absolute, so the comparison does not hang on how the path was spelled.
    let launched = argv.map(|p| std::path::absolute(&p).unwrap_or(p));
    let _ = LAUNCHED.set(launched.clone());
    let saved = std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str::<Saved>(&s).ok());
    choose(launched, saved, Path::is_file)
}

fn choose(launched: Option<PathBuf>, saved: Option<Saved>, exists: impl Fn(&Path) -> bool) -> Restore {
    match saved {
        Some(s) if (launched.is_none() || s.launched == launched) && exists(&s.file) => Restore { file: Some(s.file), view: s.view },
        _ => Restore { file: launched, view: None },
    }
}

/// `file` is open, seen from `view`.
pub fn save(file: &Path, view: View) {
    let saved = Saved { file: file.to_path_buf(), view: Some(view), launched: LAUNCHED.get().cloned().flatten() };
    let path = path();
    let Some(dir) = path.parent() else { return };
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, serde_json::to_vec(&saved).unwrap_or_default()))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        log::warn!("[model] cannot save the open model to {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: View = View { pivot: [0.1, 0.0, -0.2], yaw: 1.0, pitch: 0.3, distance: 2.5 };

    fn saved(file: &str, launched: Option<&str>) -> Saved {
        Saved { file: file.into(), view: Some(VIEW), launched: launched.map(PathBuf::from) }
    }

    fn all_exist(_: &Path) -> bool {
        true
    }

    fn restored(file: &str) -> Restore {
        Restore { file: Some(file.into()), view: Some(VIEW) }
    }

    fn framed(file: Option<&str>) -> Restore {
        Restore { file: file.map(PathBuf::from), view: None }
    }

    #[test]
    fn the_same_command_line_reopens_where_it_was() {
        // Launched on a.stl, stepped to b.stl, restarted with the same argv.
        assert_eq!(choose(Some("/m/a.stl".into()), Some(saved("/m/b.stl", Some("/m/a.stl"))), all_exist), restored("/m/b.stl"));
        // Plain launch, opened b.stl, plain relaunch.
        assert_eq!(choose(None, Some(saved("/m/b.stl", None)), all_exist), restored("/m/b.stl"));
        // A plain launch (the launcher, login) picks up whatever was open last.
        assert_eq!(choose(None, Some(saved("/m/b.stl", Some("/m/a.stl"))), all_exist), restored("/m/b.stl"));
    }

    #[test]
    fn a_command_line_naming_another_model_wins() {
        assert_eq!(choose(Some("/m/c.stl".into()), Some(saved("/m/b.stl", Some("/m/a.stl"))), all_exist), framed(Some("/m/c.stl")));
        assert_eq!(choose(Some("/m/c.stl".into()), Some(saved("/m/b.stl", None)), all_exist), framed(Some("/m/c.stl")));
    }

    #[test]
    fn nothing_saved_opens_what_the_command_line_names() {
        assert_eq!(choose(Some("/m/c.stl".into()), None, all_exist), framed(Some("/m/c.stl")));
        assert_eq!(choose(None, None, all_exist), framed(None));
    }

    #[test]
    fn a_saved_model_that_is_gone_is_skipped() {
        let gone = |p: &Path| p != Path::new("/m/b.stl");
        assert_eq!(choose(Some("/m/a.stl".into()), Some(saved("/m/b.stl", Some("/m/a.stl"))), gone), framed(Some("/m/a.stl")));
        assert_eq!(choose(None, Some(saved("/m/b.stl", None)), gone), framed(None));
    }

    #[test]
    fn the_file_reads_back() {
        let s = saved("/m/b.stl", Some("/m/a.stl"));
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&text).unwrap(), s);
    }
}
