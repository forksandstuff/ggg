+++
title = "ggg run"
weight = 6
+++

```
ggg run [--with-export-templates] [--godot <path>] [<godot-args>...]
```

Runs the current project using the Godot version declared in `ggg.toml`. If that version is not yet cached, it is downloaded first. A different executable can be supplied with `--godot` or `GGG_GODOT_EXECUTABLE` instead.

Unlike [`ggg edit`](@/docs/reference/commands/edit.md), this launches Godot in game mode rather than opening the editor. Use it to run your project from the terminal, for example in a CI environment or to quickly test without opening the full editor.

## Usage

```bash
ggg run
```

Run this from your project directory. ggg flags must come before any Godot arguments. Any arguments after the ggg flags are forwarded verbatim to Godot:

```bash
ggg run --headless
ggg run --headless --script res://tests/run_tests.gd
ggg run --with-export-templates --headless
```

## Flags

**`--with-export-templates`:** download and install export templates for the declared Godot version before running, regardless of the `export_templates` setting in `ggg.toml`. Does not modify `ggg.toml`.

If `export_templates = true` is set in `ggg.toml`, templates are always ensured without needing this flag.

**`--godot <path>`:** use the Godot executable at `<path>` instead of the managed version declared in `ggg.toml`. No engine is downloaded or cached. The path must exist and be a regular file, otherwise `ggg run` fails with an error naming the flag. When this flag is set it takes precedence over the `GGG_GODOT_EXECUTABLE` environment variable.

## Exit status

`ggg run` exits with the exit code of the Godot process, so scripts and CI jobs can check the result directly, for example a test runner that calls `get_tree().quit(1)` on failure:

```bash
ggg run --headless --script res://tests/run_tests.gd || echo "tests failed"
```

- **Godot's exit code** when Godot ran. On Windows this includes crash codes such as `0xC0000005`.
- **128 + N** on Linux and macOS when Godot was killed by signal N (e.g. 134 for SIGABRT, 137 for SIGKILL), the usual shell convention.
- **2** when ggg itself failed and Godot never started, e.g. a missing or invalid `ggg.toml`, a failed download, or an invalid `--godot` path.

While Godot runs, ggg waits for it to finish. Pressing Ctrl+C is left to Godot: ggg does not exit before Godot does, so Godot's exit code is still reported. On Linux and macOS, termination signals sent to ggg itself (SIGTERM, SIGHUP, SIGINT when not run from a terminal, ...) are forwarded to Godot, so stopping ggg, for example from a CI job or a container runtime, also stops Godot.

## Notes

- Requires a `ggg.toml` in the current directory.
- Downloads and caches the declared Godot version on first run. Subsequent runs start immediately from the cache. (Without a `--godot` flag or `GGG_GODOT_EXECUTABLE` override.)
- Does not run `ggg sync` first. If you have just added or updated dependencies, run [`ggg sync`](@/docs/reference/commands/sync.md) beforehand.

## See also

- [`ggg edit`](@/docs/reference/commands/edit.md): opens the project in the Godot editor instead
- [`ggg sync`](@/docs/reference/commands/sync.md): installs dependencies before running
