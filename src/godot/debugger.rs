//! A minimal server for Godot's remote debugger protocol, used by
//! `ggg run --watch`.
//!
//! A game started with `--remote-debug tcp://<host>:<port>` connects to that
//! address, normally the editor's. Every packet is a little-endian `u32`
//! length followed by an `Array` of `[name, thread_id, data]` encoded like
//! Godot's `encode_variant` (`core/io/marshalls.cpp`). ggg sends
//! `reload_scripts`, the message behind the editor's "Synchronize Script
//! Changes", and keeps the game from pausing at breakpoints and script errors,
//! as it would without a debugger. Everything else the game sends is dropped.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};

/// `Thread::MAIN_ID` in Godot.
const MAIN_THREAD_ID: i64 = 1;

/// The subset of Godot's `Variant` that ggg sends.
enum Variant {
    Bool(bool),
    Int(i64),
    String(String),
    Array(Vec<Variant>),
}

impl Variant {
    /// Append the `encode_variant` form of `self` to `out`: a `u32` header
    /// holding the type, then the value.
    fn encode(&self, out: &mut Vec<u8>) {
        let mut put = |word: u32| out.extend_from_slice(&word.to_le_bytes());
        match self {
            Variant::Bool(b) => {
                put(1);
                put(u32::from(*b));
            }
            Variant::Int(i) => {
                put(2 | 1 << 16); // INT, flagged as 64-bit.
                out.extend_from_slice(&i.to_le_bytes());
            }
            Variant::String(s) => {
                put(4);
                put(s.len() as u32);
                out.extend_from_slice(s.as_bytes());
                out.resize(out.len().next_multiple_of(4), 0);
            }
            Variant::Array(items) => {
                put(28);
                put(items.len() as u32);
                for item in items {
                    item.encode(out);
                }
            }
        }
    }
}

/// A packet carrying the message `name` to the game thread `thread_id`.
fn packet(name: &str, thread_id: i64, data: Vec<Variant>) -> Vec<u8> {
    let mut payload = Vec::new();
    Variant::Array(vec![
        Variant::String(name.to_string()),
        Variant::Int(thread_id),
        Variant::Array(data),
    ])
    .encode(&mut payload);
    [(payload.len() as u32).to_le_bytes().as_slice(), &payload].concat()
}

/// The game thread that sent `payload` if it is a `debug_enter` message: that
/// thread is paused until it is told to `continue`.
fn debug_enter_thread(payload: &[u8]) -> Option<i64> {
    let mut prefix = [28u32, 3].map(u32::to_le_bytes).concat();
    Variant::String("debug_enter".to_string()).encode(&mut prefix);
    let rest = payload.strip_prefix(prefix.as_slice())?;
    let header = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
    if header & 1 << 16 != 0 {
        Some(i64::from_le_bytes(rest.get(4..12)?.try_into().ok()?))
    } else {
        Some(i32::from_le_bytes(rest.get(4..8)?.try_into().ok()?).into())
    }
}

/// A debugger server on a free loopback port. Games connect to [`Self::uri`]
/// and stay connected until they exit.
pub struct DebugServer {
    uri: String,
    games: Arc<Mutex<Vec<TcpStream>>>,
}

impl DebugServer {
    /// Listen for games and accept them in a background thread.
    pub fn start() -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .context("failed to open a port for the Godot debugger")?;
        let uri = format!("tcp://{}", listener.local_addr()?);
        let games = Arc::new(Mutex::new(Vec::new()));
        let accepted = Arc::clone(&games);
        thread::spawn(move || {
            for game in listener.incoming().flatten() {
                let _ = serve(game, &accepted);
            }
        });
        Ok(Self { uri, games })
    }

    /// The value for Godot's `--remote-debug` argument.
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Ask the connected games to reload the scripts at `res_paths`.
    pub fn reload_scripts(&self, res_paths: &[String]) {
        let data = res_paths.iter().cloned().map(Variant::String).collect();
        let packet = packet("reload_scripts", MAIN_THREAD_ID, data);
        let mut games = self.games.lock().unwrap();
        games.retain_mut(|game| game.write_all(&packet).is_ok());
    }
}

/// Turn off a new game's breakpoints and error breaks, then read what it
/// sends in a thread of its own, answering a pause with `continue`.
fn serve(mut game: TcpStream, games: &Mutex<Vec<TcpStream>>) -> io::Result<()> {
    games.lock().unwrap().push(game.try_clone()?);
    for setting in ["set_skip_breakpoints", "set_ignore_error_breaks"] {
        game.write_all(&packet(setting, MAIN_THREAD_ID, vec![Variant::Bool(true)]))?;
    }
    let mut writer = game.try_clone()?;
    thread::spawn(move || -> io::Result<()> {
        let mut len = [0u8; 4];
        while game.read_exact(&mut len).is_ok() {
            let mut payload = vec![0u8; u32::from_le_bytes(len) as usize];
            game.read_exact(&mut payload)?;
            if let Some(thread_id) = debug_enter_thread(&payload) {
                writer.write_all(&packet("continue", thread_id, Vec::new()))?;
            }
        }
        Ok(())
    });
    Ok(())
}

// --- tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// Read one packet from `game` and return its payload.
    fn receive(game: &mut TcpStream) -> Vec<u8> {
        let mut len = [0u8; 4];
        game.read_exact(&mut len).unwrap();
        let mut payload = vec![0u8; u32::from_le_bytes(len) as usize];
        game.read_exact(&mut payload).unwrap();
        payload
    }

    /// Connect a stand-in game and skip the two settings sent to it.
    fn connect(server: &DebugServer) -> TcpStream {
        let mut game = TcpStream::connect(server.uri().trim_start_matches("tcp://")).unwrap();
        for setting in ["set_skip_breakpoints", "set_ignore_error_breaks"] {
            let expected = packet(setting, MAIN_THREAD_ID, vec![Variant::Bool(true)]);
            assert_eq!(receive(&mut game), expected[4..]);
        }
        game
    }

    #[test]
    fn reload_scripts_is_encoded_like_godot() {
        // ["reload_scripts", 1, ["res://a.gd"]], laid out by hand.
        let mut payload = words(&[28, 3, 4, 14]);
        payload.extend_from_slice(b"reload_scripts\0\0");
        payload.extend(words(&[2 | 1 << 16, 1, 0, 28, 1, 4, 10]));
        payload.extend_from_slice(b"res://a.gd\0\0");

        let server = DebugServer::start().unwrap();
        let mut game = connect(&server);
        server.reload_scripts(&["res://a.gd".to_string()]);
        assert_eq!(receive(&mut game), payload);
    }

    #[test]
    fn a_paused_game_is_told_to_continue() {
        // Godot sends thread ids that fit in 32 bits without the 64-bit flag.
        let mut payload = words(&[28, 3]);
        Variant::String("debug_enter".to_string()).encode(&mut payload);
        payload.extend(words(&[2, 7, 28, 0]));

        let server = DebugServer::start().unwrap();
        let mut game = connect(&server);
        game.write_all(&words(&[payload.len() as u32])).unwrap();
        game.write_all(&payload).unwrap();
        assert_eq!(receive(&mut game), packet("continue", 7, Vec::new())[4..]);
    }
}
