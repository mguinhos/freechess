//! The message protocol between the UI and the chess delegate.
//!
//! # What the delegate is for
//!
//! The delegate is the one place in the app that holds something private: the
//! player's signing key. Contracts are public by construction — every peer near
//! their ring location can read them — so the key can never live there. The
//! delegate runs locally, inside the user's own Freenet core, and its secret
//! store is what survives across browser sessions and page reloads.
//!
//! # Why the seed comes back to the UI
//!
//! The delegate stores the key and hands it to the UI on request; signing then
//! happens in the UI. Two reasons:
//!
//! * **The delegate cannot generate keys.** It runs on
//!   `wasm32-unknown-unknown` under wasmtime, which has no `getrandom` backend
//!   — pulling one in produces wasm-bindgen imports the host cannot resolve
//!   (freenet/river#241). So the UI, which has `crypto.getRandomValues`,
//!   generates the seed and hands it to the delegate to keep.
//! * **Export has to be possible.** An account *is* its key; letting the user
//!   move it between devices means the key must be retrievable by definition.
//!
//! The honest framing is therefore that the delegate is durable, private
//! storage for the key — not a signing boundary that keeps the key from the
//! page. A single game move needs several signatures (the move, the lobby
//! snapshot), and routing each through an async round-trip would buy no real
//! isolation given export exists anyway.

use serde::{Deserialize, Serialize};

/// Secret-store key under which the account seed is filed.
pub const ACCOUNT_SEED_KEY: &[u8] = b"freechess:account-seed:v1";

/// Secret-store key for the locally remembered nickname.
pub const NICKNAME_KEY: &[u8] = b"freechess:nickname:v1";

/// Code hashes of delegates that used to hold accounts, newest first.
///
/// # Why this list has to exist
///
/// A delegate's key is `BLAKE3(BLAKE3(wasm) ‖ params)`, and the account seed —
/// the player's whole identity, their rating and their history — is filed under
/// that key. So *any* change to the delegate WASM, down to a transitive
/// dependency bump, files future secrets under a new key and leaves the old
/// ones unreachable. Every player would silently become a brand-new player.
///
/// Unlike a moved contract address, this is not recoverable by republishing.
/// The only remedy is for the client to know where to look, which is what this
/// list is for: when the current delegate has no account, the client asks each
/// of these in turn and adopts the first seed it finds.
///
/// # Adding an entry
///
/// Record the OUTGOING hash *before* the change alters the WASM — afterwards
/// you can only compute the new one, which is useless:
///
/// ```text
/// cargo run -p delegate-key -- target/wasm32-unknown-unknown/release/chess_delegate.wasm
/// ```
///
/// `cargo make check-delegate-migration` fails the build when the built WASM
/// stops matching [`CURRENT_DELEGATE_CODE_HASH`] without a new entry here, so
/// the mistake cannot ship quietly.
///
/// # Limits
///
/// Reaching an old delegate needs the node to still hold its WASM, registered
/// in some earlier session. A player whose node never ran the old delegate has
/// nothing to recover, and no list can change that.
pub const LEGACY_DELEGATE_CODE_HASHES: &[[u8; 32]] = &[];

/// The code hash the current delegate build is expected to have.
///
/// Its only job is to make an unnoticed change loud: `check-delegate-migration`
/// compares the built WASM against it, so re-keying the delegate becomes a
/// build failure rather than a silent loss of every account.
pub const CURRENT_DELEGATE_CODE_HASH: [u8; 32] = [
    0xc1, 0x70, 0x4d, 0x6b, 0xdc, 0xdb, 0x50, 0x78, 0x4b, 0xdc, 0xed, 0xf6, 0x28, 0x4e, 0xb3, 0x13,
    0x63, 0xb7, 0x9e, 0x85, 0x5e, 0x5d, 0xe9, 0xd8, 0xcc, 0xb4, 0x69, 0x75, 0xc8, 0x1e, 0x7e, 0x3a,
];

/// The delegate key a given code hash produces, mirroring freenet-stdlib's
/// `generate_id`.
///
/// FreeChess registers its delegate with empty parameters, so the derivation
/// collapses to a second BLAKE3 over the code hash. Kept here — rather than
/// reaching for stdlib types — so it stays available to the contracts and to
/// tests, and so the one line that must match upstream is written down once.
pub fn delegate_key_for(code_hash: &[u8; 32]) -> [u8; 32] {
    *blake3::hash(code_hash).as_bytes()
}

/// Requests the UI sends to the delegate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChessDelegateRequest {
    /// Fetch the stored account, if there is one. The UI generates a seed and
    /// calls [`StoreAccount`](Self::StoreAccount) when this comes back empty.
    GetAccount,
    /// Store an account seed — used both for a freshly generated account and
    /// for one the user imported.
    StoreAccount {
        seed: [u8; 32],
    },
    /// Forget the stored account. Used when switching identities; the caller is
    /// responsible for having exported it first.
    ClearAccount,
    /// Remember the nickname locally so a new session can pre-fill it. The
    /// authoritative copy lives in the player contract.
    SetNickname {
        nickname: String,
    },
    GetNickname,
}

/// Responses the delegate sends back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChessDelegateResponse {
    /// The stored account seed, or `None` if this device has no account yet.
    Account {
        seed: Option<[u8; 32]>,
    },
    /// Acknowledgement that a write landed.
    Stored,
    Nickname {
        nickname: Option<String>,
    },
    Error {
        message: String,
    },
}

impl ChessDelegateRequest {
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        ciborium::ser::into_writer(self, &mut out).map_err(|e| e.to_string())?;
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        ciborium::de::from_reader(bytes).map_err(|e| e.to_string())
    }
}

impl ChessDelegateResponse {
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        ciborium::ser::into_writer(self, &mut out).map_err(|e| e.to_string())?;
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        ciborium::de::from_reader(bytes).map_err(|e| e.to_string())
    }
}
