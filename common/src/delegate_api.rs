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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        for req in [
            ChessDelegateRequest::GetAccount,
            ChessDelegateRequest::StoreAccount { seed: [7u8; 32] },
            ChessDelegateRequest::ClearAccount,
            ChessDelegateRequest::SetNickname {
                nickname: "magnus".to_string(),
            },
            ChessDelegateRequest::GetNickname,
        ] {
            let bytes = req.to_bytes().expect("encodes");
            assert_eq!(ChessDelegateRequest::from_bytes(&bytes).unwrap(), req);
        }
    }

    #[test]
    fn responses_round_trip() {
        for resp in [
            ChessDelegateResponse::Account {
                seed: Some([3u8; 32]),
            },
            ChessDelegateResponse::Account { seed: None },
            ChessDelegateResponse::Stored,
            ChessDelegateResponse::Nickname {
                nickname: Some("hikaru".to_string()),
            },
            ChessDelegateResponse::Error {
                message: "nope".to_string(),
            },
        ] {
            let bytes = resp.to_bytes().expect("encodes");
            assert_eq!(ChessDelegateResponse::from_bytes(&bytes).unwrap(), resp);
        }
    }

    #[test]
    fn a_truncated_payload_is_an_error_not_a_panic() {
        assert!(ChessDelegateRequest::from_bytes(&[0xff, 0x01]).is_err());
    }
}
