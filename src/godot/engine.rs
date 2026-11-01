//! High-level operations on Godot engine binaries.
//!
//! This module provides two primitives that commands compose together:
//!
//! - [`ensure`] - guarantee a release is present in the cache, downloading it
//!   if necessary. Returns the path to the executable.
//! - [`resolve`] - like [`ensure`], but honours a user-supplied executable
//!   override (`--godot` flag or `GGG_GODOT_EXECUTABLE`) first. When an
//!   override is given, ggg does not manage the binary at all.
//! - [`launch`] - run a Godot executable with the given arguments, forwarding
//!   stdin, stdout, and stderr to the terminal. Returns the process exit status.
//! - [`exit_code`] - map that exit status to the code ggg itself exits with,
//!   so `ggg run` / `ggg edit` report Godot's result to the caller.
//!
//! `ensure` and `launch` are intentionally separate: `ggg sync` needs `ensure`
//! but not `launch`; `ggg edit` and `ggg run` use both in sequence. `resolve`
//! is the entry point commands call so the override precedence lives in one
//! place.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

use anyhow::{Context, Result, bail};

use super::cache::GodotCache;
use super::download;
use super::release::GodotRelease;

// ---------------------------------------------------------------------------
// Executable override
// ---------------------------------------------------------------------------

/// Resolve which Godot executable to use, applying the override precedence:
///
/// ```text
/// --godot flag  >  GGG_GODOT_EXECUTABLE  >  managed default
/// ```
///
/// With no override, delegates to [`ensure`] so the pinned release is
/// downloaded/cached as usual. When an override is present it is validated
/// (must exist and be a regular file) and returned as-is; ggg never downloads
/// or manages the binary in that case.
pub fn resolve(
    release: &GodotRelease,
    cache: &GodotCache,
    cli_path: Option<&Path>,
) -> Result<PathBuf> {
    match cli_path.map(Path::to_path_buf).or(env_executable()) {
        Some(path) => {
            let via = if cli_path.is_some() {
                "`--godot`"
            } else {
                "`$GGG_GODOT_EXECUTABLE`"
            };
            validate_executable(&path, via)?;
            Ok(path)
        }
        None => ensure(release, cache),
    }
}

/// Ensure `release` is present in `cache`, downloading and installing it if
/// not. Returns the path to the Godot executable.
fn ensure(release: &GodotRelease, cache: &GodotCache) -> Result<PathBuf> {
    if cache.contains(release) {
        return cache.executable_path(release);
    }

    let archive = download::download_release(release)
        .with_context(|| format!("failed to download Godot {}", release.tag()))?;

    let executable = cache
        .install(release, &archive)
        .with_context(|| format!("failed to install Godot {}", release.tag()))?;

    // Clean up the downloaded archive now that it has been extracted.
    let _ = std::fs::remove_file(&archive);

    Ok(executable)
}

/// The path from `GGG_GODOT_EXECUTABLE`, or `None` when the variable is unset
/// or set to an empty string (an empty value is treated as no override).
fn env_executable() -> Option<PathBuf> {
    std::env::var(crate::envvars::GODOT_EXECUTABLE_ENV_VAR)
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Guardrail: an override must point at a real, regular file. Produces a clear
/// error naming where the path came from instead of panicking or falling back
/// to a managed download.
fn validate_executable(path: &Path, via: &str) -> Result<()> {
    if !path.exists() {
        bail!(
            "the Godot executable from {} does not exist: {}",
            via,
            path.display()
        );
    }
    if !path.is_file() {
        bail!(
            "the Godot executable from {} is not a file: {}",
            via,
            path.display()
        );
    }
    Ok(())
}

/// Launch the Godot executable at `path` with the given `args`, inheriting
/// stdin, stdout, and stderr from the current process.
///
/// The path must have been obtained from [`ensure`] - passing an arbitrary
/// path that does not point to a real executable is a programmer error.
///
/// While Godot runs, ggg stays alive until Godot exits so its exit status is
/// never lost: Ctrl+C is left to Godot, and other termination signals sent to
/// ggg are forwarded to it (see [`spawn_and_wait`]). This mirrors how
/// `uv run` supervises its child.
///
/// Returns the exit status of the Godot process.
pub fn launch(executable: &Path, args: &[String]) -> Result<ExitStatus> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    spawn_and_wait(&mut command, executable)
}

/// Spawn `command`, attributing a failure to the Godot executable at `path`.
fn spawn(command: &mut Command, path: &Path) -> Result<Child> {
    command
        .spawn()
        .with_context(|| format!("failed to launch Godot at {}", path.display()))
}

/// Map Godot's exit status to the code ggg should exit with, following uv:
///
/// - a normal exit propagates Godot's code unchanged (on Windows this includes
///   NTSTATUS codes such as `0xC0000005`, which `std::process::exit` passes
///   through as-is);
/// - on Unix, death by signal `N` becomes `128 + N`, the shell convention;
/// - anything else is reported as a generic failure (`1`).
pub fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

/// Spawn `command` and wait for it to exit while forwarding signals sent to
/// ggg. The handlers are installed before spawning, so a signal arriving
/// while Godot starts up is forwarded rather than killing ggg.
///
/// In an interactive terminal, Ctrl+C sends SIGINT to the whole foreground
/// process group, so Godot already receives it; ggg ignores it rather than
/// sending a second one (which some programs treat as "exit immediately").
/// When stdin is not a terminal, or Godot left ggg's process group, nobody
/// else delivers it, so ggg forwards SIGINT after a short grace period.
///
/// Every other signal in the list is forwarded unconditionally: a terminal
/// does not relay e.g. SIGTERM to children, and in a container where ggg is
/// PID 1 Godot would otherwise be unkillable.
///
/// The set of signals and the SIGINT heuristic match `uv run`.
#[cfg(unix)]
fn spawn_and_wait(command: &mut Command, path: &Path) -> Result<ExitStatus> {
    use signal_hook::consts::signal::*;
    use signal_hook::iterator::Signals;
    use std::io::IsTerminal;

    #[allow(unused_mut)]
    let mut forwarded = vec![
        SIGINT, SIGTERM, SIGHUP, SIGQUIT, SIGUSR1, SIGUSR2, SIGALRM, SIGWINCH, SIGPIPE,
    ];
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
        target_os = "illumos",
    ))]
    forwarded.push(SIGINFO);

    let mut signals = Signals::new(&forwarded).context("failed to install signal handlers")?;
    let handle = signals.handle();
    let mut child = match spawn(command, path) {
        Ok(child) => child,
        Err(err) => {
            handle.close();
            return Err(err);
        }
    };
    let pid = child.id() as libc::pid_t;
    let interactive = std::io::stdin().is_terminal();

    let forwarder = std::thread::spawn(move || {
        for signal in signals.forever() {
            if signal == SIGINT {
                // SAFETY: getpgid has no memory-safety preconditions.
                let same_group = unsafe { libc::getpgid(pid) == libc::getpgid(0) };
                if interactive && same_group {
                    continue;
                }
                // The terminal may still be delivering its own SIGINT; give
                // Godot a moment before sending another one.
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            // SAFETY: kill has no memory-safety preconditions.
            unsafe { libc::kill(pid, signal) };
        }
    });

    let status = child.wait();
    handle.close();
    let _ = forwarder.join();
    status.context("failed to wait for Godot to exit")
}

/// Spawn `command` and wait for it to exit, ignoring Ctrl+C in ggg.
///
/// The console delivers Ctrl+C to every process attached to it, so Godot
/// receives it directly; ggg just has to survive long enough to report
/// Godot's exit code. There is no portable way to forward other console
/// events, matching `uv run` on Windows.
#[cfg(windows)]
fn spawn_and_wait(command: &mut Command, path: &Path) -> Result<ExitStatus> {
    // Fails only if a handler is already installed, in which case Ctrl+C is
    // already not terminating ggg.
    let _ = ctrlc::set_handler(|| {});
    spawn(command, path)?
        .wait()
        .context("failed to wait for Godot to exit")
}

// --- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::godot::cache::GodotCache;
    use serial_test::serial;

    fn temp_file() -> tempfile::NamedTempFile {
        tempfile::NamedTempFile::new().unwrap()
    }

    // --- env parsing --------------------------------------------------------

    #[test]
    #[serial]
    fn env_executable_ignores_empty_value() {
        // An explicitly empty variable is treated as "no override", mirroring
        // how uv treats an empty UV_PYTHON rather than producing a do-nothing
        // path.
        unsafe {
            std::env::set_var(crate::envvars::GODOT_EXECUTABLE_ENV_VAR, "");
        }
        assert_eq!(env_executable(), None);
        unsafe { std::env::remove_var(crate::envvars::GODOT_EXECUTABLE_ENV_VAR) };
    }

    #[test]
    #[serial]
    fn env_executable_reads_nonempty_value() {
        unsafe {
            std::env::set_var(
                crate::envvars::GODOT_EXECUTABLE_ENV_VAR,
                "/tmp/custom-godot",
            );
        }
        assert_eq!(env_executable(), Some(PathBuf::from("/tmp/custom-godot")));
        unsafe { std::env::remove_var(crate::envvars::GODOT_EXECUTABLE_ENV_VAR) };
    }

    #[test]
    #[serial]
    fn env_executable_none_when_unset() {
        unsafe { std::env::remove_var(crate::envvars::GODOT_EXECUTABLE_ENV_VAR) };
        assert_eq!(env_executable(), None);
    }

    // --- validation --------------------------------------------------------

    #[test]
    fn validate_accepts_existing_file() {
        let file = temp_file();
        assert!(validate_executable(file.path(), "`--godot`").is_ok());
    }

    #[test]
    fn validate_rejects_missing_path_and_names_source() {
        let missing = PathBuf::from("/does/not/exist/godot");
        let err = validate_executable(&missing, "`--godot`")
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not exist"), "err: {err}");
        assert!(err.contains("`--godot`"), "err: {err}");
        assert!(err.contains("/does/not/exist/godot"), "err: {err}");
    }

    #[test]
    fn validate_rejects_directory_and_names_source() {
        let dir = tempfile::tempdir().unwrap();
        let err = validate_executable(dir.path(), "`$GGG_GODOT_EXECUTABLE`")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a file"), "err: {err}");
        assert!(err.contains("`$GGG_GODOT_EXECUTABLE`"), "err: {err}");
    }

    // --- exit code ---------------------------------------------------------

    #[cfg(unix)]
    fn status(raw: i32) -> ExitStatus {
        std::os::unix::process::ExitStatusExt::from_raw(raw)
    }

    #[cfg(windows)]
    fn status(raw: u32) -> ExitStatus {
        std::os::windows::process::ExitStatusExt::from_raw(raw)
    }

    #[test]
    fn exit_code_propagates_success_and_failure_codes() {
        // Unix encodes the exit code in the high byte of the wait status.
        #[cfg(unix)]
        let (ok, failed) = (status(0), status(42 << 8));
        #[cfg(windows)]
        let (ok, failed) = (status(0), status(42));

        assert_eq!(exit_code(ok), 0);
        assert_eq!(exit_code(failed), 42);
    }

    #[cfg(unix)]
    #[test]
    fn exit_code_maps_signal_to_128_plus_n() {
        // A raw wait status of N (low bits) means "killed by signal N".
        assert_eq!(exit_code(status(libc::SIGTERM)), 128 + libc::SIGTERM);
        assert_eq!(exit_code(status(libc::SIGKILL)), 128 + libc::SIGKILL);
    }

    #[cfg(windows)]
    #[test]
    fn exit_code_preserves_ntstatus_codes() {
        // STATUS_ACCESS_VIOLATION, reported by Godot crashing on Windows.
        assert_eq!(exit_code(status(0xC000_0005)), 0xC000_0005_u32 as i32);
    }

    // --- resolve -----------------------------------------------------------

    #[test]
    fn resolve_returns_cli_override_without_managing() {
        let file = temp_file();
        let dir = tempfile::tempdir().unwrap();
        let cache = GodotCache::new(dir.path().to_path_buf());
        let release: GodotRelease = "4.3-stable".parse().unwrap();

        // With a --godot override the release is neither cached nor downloaded:
        // the empty cache is never touched and the override path is returned.
        let resolved = resolve(&release, &cache, Some(file.path())).unwrap();
        assert_eq!(resolved, file.path());
        assert!(!cache.contains(&release));
    }

    #[test]
    fn resolve_errors_on_invalid_override() {
        let dir = tempfile::tempdir().unwrap();
        let cache = GodotCache::new(dir.path().to_path_buf());
        let release: GodotRelease = "4.3-stable".parse().unwrap();
        let missing = dir.path().join("nope");

        let err = resolve(&release, &cache, Some(&missing))
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not exist"), "err: {err}");
        assert!(err.contains("`--godot`"), "err: {err}");
        assert!(!cache.contains(&release));
    }
}
