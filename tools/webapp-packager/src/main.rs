//! Signs a packaged webapp and assembles the web-container contract state.
//!
//! Runs on the build host, not in WASM. It is deliberately separate from the
//! contract: the contract only ever *verifies*, so it never needs a private key
//! or a randomness source (both of which would drag `getrandom` into the WASM
//! and break instantiation under wasmtime).
//!
//! Usage:
//!
//! ```text
//! webapp-packager --archive target/webapp/webapp.tar.xz \
//!                 --key    .freenet-publisher.key \
//!                 --version 1 \
//!                 --out-dir target/webapp
//! ```
//!
//! The publisher key is created on first run and reused afterwards. Keeping it
//! stable is what keeps the app's URL stable across releases, because the
//! contract parameters — and therefore the contract key — are that key.

use chess_core::web_container::{encode_state, signing_bytes, WebContainerMetadata};
use ed25519_dalek::{Signer, SigningKey};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

struct Args {
    archive: Option<PathBuf>,
    key_path: PathBuf,
    version: u32,
    out_dir: PathBuf,
    /// Only write the contract parameters (the publisher key), skipping the
    /// archive. The build needs these first: the webapp's contract id depends
    /// on the container WASM and the publisher key but *not* on the bundle
    /// contents, so the id can be computed before the UI is built — which is
    /// what lets the UI be built with the right `base_path` baked in.
    params_only: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut archive = None;
    let mut key_path = None;
    let mut version = None;
    let mut out_dir = None;
    let mut params_only = false;

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--archive" => archive = Some(PathBuf::from(value()?)),
            "--key" => key_path = Some(PathBuf::from(value()?)),
            "--version" => {
                version = Some(
                    value()?
                        .parse::<u32>()
                        .map_err(|e| format!("--version must be a number: {e}"))?,
                )
            }
            "--out-dir" => out_dir = Some(PathBuf::from(value()?)),
            "--params-only" => params_only = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }

    Ok(Args {
        archive,
        key_path: key_path.ok_or("--key is required")?,
        version: version.unwrap_or(1),
        out_dir: out_dir.ok_or("--out-dir is required")?,
        params_only,
    })
}

/// Load the publisher key, creating it on first use.
fn load_or_create_key(path: &Path) -> Result<SigningKey, String> {
    if path.exists() {
        let bytes =
            fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let seed: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| format!("{} is not a 32-byte key", path.display()))?;
        return Ok(SigningKey::from_bytes(&seed));
    }

    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    fs::write(path, key.to_bytes())
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    // Owner-only: this key controls what the app's URL serves.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    eprintln!("created a new publisher key at {}", path.display());
    eprintln!("KEEP IT: losing it means the app's URL can never be updated again.");
    Ok(key)
}

fn run() -> Result<(), String> {
    let args = parse_args()?;

    if args.version == 0 {
        return Err("--version must be at least 1 (0 is reserved for 'unset')".to_string());
    }

    let key = load_or_create_key(&args.key_path)?;

    fs::create_dir_all(&args.out_dir)
        .map_err(|e| format!("could not create {}: {e}", args.out_dir.display()))?;
    let params_path = args.out_dir.join("webapp.params");
    // Contract parameters are the raw 32-byte verifying key.
    fs::write(&params_path, key.verifying_key().to_bytes())
        .map_err(|e| format!("could not write {}: {e}", params_path.display()))?;

    if args.params_only {
        println!("params:    {}", params_path.display());
        println!(
            "publisher: {}",
            bs58::encode(key.verifying_key().to_bytes()).into_string()
        );
        return Ok(());
    }

    let archive_path = args
        .archive
        .ok_or("--archive is required unless --params-only is given")?;
    let webapp = fs::read(&archive_path)
        .map_err(|e| format!("could not read {}: {e}", archive_path.display()))?;
    if webapp.is_empty() {
        return Err(format!("{} is empty", archive_path.display()));
    }

    let signature = key.sign(&signing_bytes(args.version, &webapp));

    let metadata = WebContainerMetadata {
        version: args.version,
        signature,
    };
    let mut metadata_cbor = Vec::new();
    ciborium::ser::into_writer(&metadata, &mut metadata_cbor)
        .map_err(|e| format!("could not encode metadata: {e}"))?;

    let state = encode_state(&metadata_cbor, &webapp);

    let state_path = args.out_dir.join("webapp.state");
    fs::write(&state_path, &state)
        .map_err(|e| format!("could not write {}: {e}", state_path.display()))?;

    println!("version:   {}", args.version);
    println!("archive:   {} bytes", webapp.len());
    println!(
        "state:     {} ({} bytes)",
        state_path.display(),
        state.len()
    );
    println!("params:    {}", params_path.display());
    println!(
        "publisher: {}",
        bs58::encode(key.verifying_key().to_bytes()).into_string()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("webapp-packager: {e}");
            ExitCode::FAILURE
        }
    }
}
