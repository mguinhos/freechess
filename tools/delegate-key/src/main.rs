//! Print a delegate WASM's code hash and the key derived from it.
//!
//! Needed whenever the delegate is about to change: the OLD key has to be
//! recorded in [`chess_core::delegate_api::LEGACY_DELEGATE_CODE_HASHES`] before
//! the change lands, or the players' account seeds become unreachable — they are
//! stored under the key, and the key is a hash of the bytes.
//!
//! Mirrors freenet-stdlib's derivation exactly. With empty parameters (which is
//! what FreeChess uses) that is:
//!
//! ```text
//! code_hash = BLAKE3(wasm)
//! key       = BLAKE3(code_hash)
//! ```

use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: delegate-key <path/to/chess_delegate.wasm>");
        return ExitCode::FAILURE;
    };
    let wasm = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("could not read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let code_hash = *blake3::hash(&wasm).as_bytes();
    let key = *blake3::hash(&code_hash).as_bytes();

    println!("code_hash_hex   {}", hex(&code_hash));
    println!("code_hash_b58   {}", bs58::encode(code_hash).into_string());
    println!("delegate_key    {}", bs58::encode(key).into_string());
    println!();
    println!("To retire this delegate, add its code hash to");
    println!("LEGACY_DELEGATE_CODE_HASHES in common/src/delegate_api.rs:");
    println!();
    println!("    hex!(\"{}\"),", hex(&code_hash));
    ExitCode::SUCCESS
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
