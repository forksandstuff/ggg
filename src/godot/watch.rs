//! `ggg run --watch`: run the game and reload GDScript files as they change.
//!
//! The game is started with `--remote-debug` pointing at ggg's own
//! [`DebugServer`]. Once saves under the project directory stop for
//! [`DEBOUNCE`], the server asks the game to reload the `.gd` files that
//! changed.

use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;

use anyhow::{Context, Result};
use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};

use super::debugger::DebugServer;
use super::engine;

/// How long saves must stop before a reload. Editors often write a file in
/// several steps.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Run Godot at `executable` with `args`, reloading the scripts of the
/// project at `project_dir` (the `res://` root) until the game exits.
pub fn run(executable: &Path, args: &[String], project_dir: &Path) -> Result<ExitStatus> {
    let server = DebugServer::start()?;
    // Godot's own options must come before a `--` that starts the game's.
    let mut godot_args = vec!["--remote-debug".to_string(), server.uri().to_string()];
    godot_args.extend_from_slice(args);

    let root = project_dir.to_path_buf();
    let mut debouncer = new_debouncer(DEBOUNCE, move |events: DebounceEventResult| {
        let scripts: Vec<String> = events
            .into_iter()
            .flatten()
            .filter(|event| event.path.is_file())
            .filter_map(|event| res_path(&root, &event.path))
            .collect();
        if !scripts.is_empty() {
            eprintln!("ggg: reloading {}", scripts.join(", "));
            server.reload_scripts(&scripts);
        }
    })
    .context("failed to start the file watcher")?;
    debouncer
        .watcher()
        .watch(project_dir, RecursiveMode::Recursive)?;

    engine::launch(executable, &godot_args)
}

/// The `res://` path of the GDScript file at `path`, or `None` for other
/// files and for files outside `project_dir` or under a hidden directory such
/// as `.godot/`.
fn res_path(project_dir: &Path, path: &Path) -> Option<String> {
    if path.extension()? != "gd" {
        return None;
    }
    let parts = path
        .strip_prefix(project_dir)
        .ok()?
        .iter()
        .map(|part| part.to_str().filter(|part| !part.starts_with('.')))
        .collect::<Option<Vec<_>>>()?;
    Some(format!("res://{}", parts.join("/")))
}
