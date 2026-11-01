//! Implementation of `ggg run`.
//!
//! Resolves the Godot executable (honouring a `--godot` override or
//! `GGG_GODOT_EXECUTABLE`), then launches it against the current project. Any
//! additional arguments are forwarded verbatim to the Godot process. With
//! `--watch`, changed GDScript files are reloaded into the running game
//! ([`crate::godot::watch`]).
//!
//! Once Godot exits, ggg exits with Godot's exit code (see [`engine::exit_code`]),
//! so scripts and CI can rely on it.

use anyhow::{Context, Result};
use std::path::Path;

use crate::config::Config;
use crate::godot::cache::GodotCache;
use crate::godot::engine;
use crate::godot::export_templates;
use crate::godot::watch;

pub fn run(
    args: &[String],
    with_export_templates: bool,
    godot: Option<String>,
    watch: bool,
) -> Result<()> {
    let config = Config::load(Path::new("ggg.toml"))?;
    let cache = GodotCache::from_env()?;
    let executable = engine::resolve(
        &config.project.godot,
        &cache,
        godot.as_deref().map(Path::new),
    )?;

    if with_export_templates || config.project.export_templates {
        export_templates::ensure_export_templates(&config.project.godot)?;
    }

    let status = if watch {
        let project_dir =
            std::env::current_dir().context("failed to read the current directory")?;
        watch::run(&executable, args, &project_dir)?
    } else {
        engine::launch(&executable, args)?
    };
    std::process::exit(engine::exit_code(status));
}
