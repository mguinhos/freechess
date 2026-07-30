//! Emits the parameters and initial state for FreeChess's singleton contracts.
//!
//! The lobby is a singleton: one instance the whole app shares. A client PUTs
//! it with empty state on startup, which is enough on a small network — but on
//! the public network that first PUT has to race the client's own GET, and a
//! visitor arriving before anyone has published it sees `NotFound`.
//!
//! Publishing it once from the deployer removes that race:
//!
//! ```text
//! cargo run -p bootstrap-contracts -- --out-dir target/bootstrap
//! fdev --port <ws> publish \
//!   --code target/wasm32-unknown-unknown/release/lobby_contract.wasm \
//!   --parameters target/bootstrap/lobby.params \
//!   contract --state target/bootstrap/lobby.state
//! ```

use chess_core::lobby::{LobbyParametersV1, LobbyStateV1};
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(value, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn run() -> Result<(), String> {
    let mut out_dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--out-dir" => {
                out_dir = Some(PathBuf::from(args.next().ok_or("--out-dir needs a value")?))
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let out_dir = out_dir.ok_or("--out-dir is required")?;
    fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    // The lobby's parameters are just its namespace, so every client derives
    // the same instance without being told which one to use.
    let params = encode(&LobbyParametersV1::default())?;
    // Empty state is a valid lobby: entries are added later by the players
    // themselves, each one self-authorizing.
    let state = encode(&LobbyStateV1::default())?;

    let params_path = out_dir.join("lobby.params");
    let state_path = out_dir.join("lobby.state");
    fs::write(&params_path, &params).map_err(|e| e.to_string())?;
    fs::write(&state_path, &state).map_err(|e| e.to_string())?;

    println!(
        "lobby params: {} ({} bytes)",
        params_path.display(),
        params.len()
    );
    println!(
        "lobby state:  {} ({} bytes)",
        state_path.display(),
        state.len()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bootstrap-contracts: {e}");
            ExitCode::FAILURE
        }
    }
}
