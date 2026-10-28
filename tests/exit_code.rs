//! End-to-end tests for the exit codes of `ggg run` and `ggg edit`.
//!
//! Following uv, ggg exits with Godot's own exit code once Godot has run
//! (Unix signal deaths become 128+N), and with 2 when ggg itself fails before
//! or while launching Godot. Godot is replaced by the fake binary from
//! [`common::fake_godot_executable`], whose behaviour is chosen through
//! [`FAKE_GODOT_ENV_VAR`].

mod common;

use predicates::str::contains;

use common::wiremock::MockApi;
use common::{FAKE_GODOT_ENV_VAR, TestProject, fake_godot_executable};
use ggg::envvars::GODOT_EXECUTABLE_ENV_VAR;

/// Exit code ggg uses for its own errors.
const GGG_ERROR: i32 = 2;

/// Godot exit code used by the failure tests: if ggg wrongly launched Godot,
/// the test would see 42 instead of [`GGG_ERROR`].
const FAKE_GODOT_LAUNCHED: &str = "42";

/// A project with a `ggg.toml` whose fake Godot exits with `mode`.
fn project_with_fake_godot(mode: &str) -> TestProject {
    let mut project = TestProject::new();
    project.env(FAKE_GODOT_ENV_VAR, mode);
    project.config().write_no_seed();
    project
}

// ---------------------------------------------------------------------------
// Godot's exit code is propagated
// ---------------------------------------------------------------------------

#[test]
fn run_and_edit_propagate_godot_exit_code() {
    let exe = fake_godot_executable();
    for cmd in ["run", "edit"] {
        for code in [0, 1, 3, 42, 255] {
            let project = project_with_fake_godot(&code.to_string());
            project
                .cmd()
                .args([cmd, "--godot"])
                .arg(&exe)
                .assert()
                .code(code);
        }
    }
}

#[test]
fn run_and_edit_propagate_exit_code_with_env_override() {
    let exe = fake_godot_executable();
    for cmd in ["run", "edit"] {
        let mut project = project_with_fake_godot("7");
        project.env(GODOT_EXECUTABLE_ENV_VAR, exe.display().to_string());
        project.cmd().arg(cmd).assert().code(7);
    }
}

#[test]
fn run_propagates_exit_code_with_forwarded_godot_args() {
    // Extra arguments are forwarded to Godot and do not affect the exit code.
    let project = project_with_fake_godot("5");
    project
        .cmd()
        .args(["run", "--godot"])
        .arg(fake_godot_executable())
        .args(["--headless", "--script", "res://tests/run.gd"])
        .assert()
        .code(5);
}

#[cfg(windows)]
#[test]
fn run_propagates_ntstatus_exit_code() {
    // STATUS_ACCESS_VIOLATION, what a crashing Godot reports on Windows. The
    // full 32-bit code must survive, not be truncated to a byte.
    let code = 0xC000_0005_u32 as i32;
    let project = project_with_fake_godot(&code.to_string());
    project
        .cmd()
        .args(["run", "--godot"])
        .arg(fake_godot_executable())
        .assert()
        .code(code);
}

#[cfg(unix)]
#[test]
fn run_maps_godot_signal_death_to_128_plus_n() {
    let project = project_with_fake_godot("abort");
    project
        .cmd()
        .args(["run", "--godot"])
        .arg(fake_godot_executable())
        .assert()
        .code(128 + libc::SIGABRT);
}

// ---------------------------------------------------------------------------
// ggg's own failures exit with 2 and never launch Godot
// ---------------------------------------------------------------------------

#[test]
fn missing_godot_flag_path_exits_with_ggg_error() {
    let tmp = tempfile::tempdir().unwrap();
    for cmd in ["run", "edit"] {
        let project = project_with_fake_godot(FAKE_GODOT_LAUNCHED);
        project
            .cmd()
            .args([cmd, "--godot"])
            .arg(tmp.path().join("missing-godot.exe"))
            .assert()
            .code(GGG_ERROR)
            .stderr(contains("does not exist"));
    }
}

#[test]
fn missing_env_override_path_exits_with_ggg_error() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing-godot.exe");
    for cmd in ["run", "edit"] {
        let mut project = project_with_fake_godot(FAKE_GODOT_LAUNCHED);
        project.env(GODOT_EXECUTABLE_ENV_VAR, missing.display().to_string());
        project
            .cmd()
            .arg(cmd)
            .assert()
            .code(GGG_ERROR)
            .stderr(contains("does not exist"));
    }
}

#[test]
fn directory_override_exits_with_ggg_error() {
    let dir = tempfile::tempdir().unwrap();
    for cmd in ["run", "edit"] {
        let project = project_with_fake_godot(FAKE_GODOT_LAUNCHED);
        project
            .cmd()
            .args([cmd, "--godot"])
            .arg(dir.path())
            .assert()
            .code(GGG_ERROR)
            .stderr(contains("not a file"));
    }
}

#[test]
fn unlaunchable_override_exits_with_ggg_error() {
    // A regular file that is not a program: on Unix it lacks the executable
    // bit, on Windows it is not a valid PE image. Validation passes, so this
    // exercises the spawn failure itself.
    let dir = tempfile::tempdir().unwrap();
    let not_a_program = dir.path().join("godot.exe");
    std::fs::write(&not_a_program, "this is not Godot").unwrap();

    for cmd in ["run", "edit"] {
        let project = project_with_fake_godot(FAKE_GODOT_LAUNCHED);
        project
            .cmd()
            .args([cmd, "--godot"])
            .arg(&not_a_program)
            .assert()
            .code(GGG_ERROR)
            .stderr(contains("failed to launch Godot"));
    }
}

#[test]
fn missing_ggg_toml_exits_with_ggg_error() {
    let exe = fake_godot_executable();
    for cmd in ["run", "edit"] {
        let mut project = TestProject::new();
        project.env(FAKE_GODOT_ENV_VAR, FAKE_GODOT_LAUNCHED);
        project
            .cmd()
            .args([cmd, "--godot"])
            .arg(&exe)
            .assert()
            .code(GGG_ERROR);
    }
}

#[test]
fn malformed_ggg_toml_exits_with_ggg_error() {
    let exe = fake_godot_executable();
    for cmd in ["run", "edit"] {
        let project = project_with_fake_godot(FAKE_GODOT_LAUNCHED);
        project.write("ggg.toml", "this is [not valid toml");
        project
            .cmd()
            .args([cmd, "--godot"])
            .arg(&exe)
            .assert()
            .code(GGG_ERROR);
    }
}

#[tokio::test]
async fn engine_download_failure_exits_with_ggg_error() {
    // No override and an empty cache: ggg must download the engine, but the
    // mocked builds API has nothing mounted, so the download fails.
    let api = MockApi::start().await;
    for cmd in ["run", "edit"] {
        let mut project = TestProject::new();
        project.env_builds_api(&api);
        project.config().write_no_seed();
        project
            .cmd()
            .arg(cmd)
            .assert()
            .code(GGG_ERROR)
            .stderr(contains("failed to download Godot"));
    }
}

#[tokio::test]
async fn export_template_failure_exits_with_ggg_error() {
    // Export templates are ensured before Godot launches; when that download
    // fails, Godot must not be started.
    let api = MockApi::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let exe = fake_godot_executable();
    for cmd in ["run", "edit"] {
        let mut project = TestProject::new();
        project
            .env(FAKE_GODOT_ENV_VAR, FAKE_GODOT_LAUNCHED)
            .env_downloads_base(&api)
            .env_data_dir(data_dir.path());
        project.config().export_templates(true).write_no_seed();
        project
            .cmd()
            .args([cmd, "--godot"])
            .arg(&exe)
            .assert()
            .code(GGG_ERROR)
            .stderr(contains("export templates"));
    }
}

// ---------------------------------------------------------------------------
// Signals sent to ggg are forwarded to Godot (Unix)
// ---------------------------------------------------------------------------

/// Start `ggg run` with a fake Godot that sleeps, wait until Godot is up, send
/// `signal` to ggg (not to Godot), and return ggg's exit code.
///
/// stdin is not a terminal here, so ggg forwards SIGINT too.
#[cfg(unix)]
fn exit_code_after_signalling_ggg(signal: libc::c_int) -> Option<i32> {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let project = project_with_fake_godot("sleep");
    let mut ggg = project
        .std_cmd()
        .args(["run", "--godot"])
        .arg(fake_godot_executable())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    // Godot inherits ggg's stdout and prints `ready` once it is running, by
    // which time ggg has installed its signal handlers.
    let mut line = String::new();
    BufReader::new(ggg.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready");

    // SAFETY: kill has no memory-safety preconditions.
    let sent = unsafe { libc::kill(ggg.id() as libc::pid_t, signal) };
    assert_eq!(sent, 0, "failed to signal ggg");

    ggg.wait().unwrap().code()
}

#[cfg(unix)]
#[test]
fn sigterm_is_forwarded_to_godot() {
    // ggg survives the SIGTERM, Godot dies from the forwarded one, and ggg
    // reports Godot's death as 128+SIGTERM. Had ggg not forwarded it, Godot
    // would sleep on and ggg's own exit would carry no code at all.
    assert_eq!(
        exit_code_after_signalling_ggg(libc::SIGTERM),
        Some(128 + libc::SIGTERM)
    );
}

#[cfg(unix)]
#[test]
fn sighup_is_forwarded_to_godot() {
    assert_eq!(
        exit_code_after_signalling_ggg(libc::SIGHUP),
        Some(128 + libc::SIGHUP)
    );
}

#[cfg(unix)]
#[test]
fn sigint_is_forwarded_to_godot_when_not_interactive() {
    assert_eq!(
        exit_code_after_signalling_ggg(libc::SIGINT),
        Some(128 + libc::SIGINT)
    );
}
