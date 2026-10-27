//! End-to-end tests for `ggg run --watch`.
//!
//! A stand-in Godot, compiled at test time, does what the real game does with
//! `--remote-debug`: it connects to ggg's debugger server and reads its
//! messages. Once connected it saves scripts the way an editor would, then
//! records the reload ggg sends and exits, which ends `ggg run`.

mod common;

use predicates::str::contains;

use common::{TestProject, compile_executable};

/// A fake Godot that connects to the `--remote-debug` address, writes
/// `scripts/player.gd`, `.godot/cache.gd` and `scripts/level.tscn` once it is
/// set up, and saves the readable text of the first `reload_scripts` message
/// to `reload.txt` and its arguments to `args.txt`. It exits with 3 if no
/// reload arrives within 20 seconds.
const FAKE_GODOT: &str = r#"
use std::io::Read;
use std::net::TcpStream;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::fs::write("args.txt", args.join("\n")).unwrap();
    let at = args.iter().position(|a| a == "--remote-debug").expect("no --remote-debug");
    let mut stream = TcpStream::connect(args[at + 1].trim_start_matches("tcp://")).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    loop {
        let mut len = [0u8; 4];
        if stream.read_exact(&mut len).is_err() {
            std::process::exit(3);
        }
        let mut payload = vec![0u8; u32::from_le_bytes(len) as usize];
        stream.read_exact(&mut payload).unwrap();
        let text: String = payload
            .iter()
            .map(|&b| if b.is_ascii_graphic() { b as char } else { ' ' })
            .collect();
        if text.contains("set_ignore_error_breaks") {
            std::fs::write("scripts/player.gd", "extends Node\n# changed\n").unwrap();
            std::fs::write(".godot/cache.gd", "extends Node\n").unwrap();
            std::fs::write("scripts/level.tscn", "[gd_scene]\n").unwrap();
        }
        if text.contains("reload_scripts") {
            std::fs::write("reload.txt", text).unwrap();
            return;
        }
    }
}
"#;

fn fake_godot() -> std::path::PathBuf {
    static FAKE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    FAKE.get_or_init(|| compile_executable("fake_godot_debugger", FAKE_GODOT))
        .clone()
}

#[test]
fn run_watch_reloads_saved_scripts_in_the_game() {
    let exe = fake_godot();
    let project = TestProject::new();
    project.config().write_no_seed();
    project.write("scripts/player.gd", "extends Node\n");
    project.write(".godot/.keep", "");

    project
        .cmd()
        .args(["run", "--watch", "--godot"])
        .arg(&exe)
        .args(["--headless", "--", "--level=1"])
        .timeout(std::time::Duration::from_secs(60))
        .assert()
        .success()
        .stderr(contains("ggg: reloading res://scripts/player.gd"));

    let reload = project.read("reload.txt");
    assert!(
        reload.contains("res://scripts/player.gd"),
        "reload: {reload}"
    );
    assert!(
        !reload.contains("cache.gd"),
        "hidden dir reloaded: {reload}"
    );
    assert!(
        !reload.contains("level.tscn"),
        "non-script reloaded: {reload}"
    );

    // The debugger option comes before the user's own Godot arguments, so it
    // is never mistaken for a game argument after `--`.
    let args = project.read("args.txt");
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(args[0], "--remote-debug");
    assert!(args[1].starts_with("tcp://127.0.0.1:"), "args: {args:?}");
    assert_eq!(args[2..], ["--headless", "--", "--level=1"]);
}
