//! Require the modules embedded by the UI and expose their build directory.
//!
//! New games and fresh nodes need the game and lobby code to publish contracts.
//! Registering the private account delegate also requires its WASM module.

use std::path::PathBuf;

const EMBEDDED: [&str; 3] = ["game_contract", "lobby_contract", "chess_delegate"];

fn main() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ui/ has a parent")
        .to_path_buf();
    let wasm_dir = workspace.join("target/wasm32-unknown-unknown/release");

    let require = |name: &str| {
        let path = wasm_dir.join(format!("{name}.wasm"));
        if !path.exists() {
            panic!(
                "missing {}\n\n\
                 The UI needs the contracts built first:\n\
                 \n    cargo make build-contracts\n",
                path.display()
            );
        }
        println!("cargo:rerun-if-changed={}", path.display());
    };

    for name in EMBEDDED {
        require(name);
    }

    println!("cargo:rustc-env=CHESS_WASM_DIR={}", wasm_dir.display());
    println!("cargo:rerun-if-changed=build.rs");
}
