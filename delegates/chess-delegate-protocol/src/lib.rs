//! This separate crate keeps protocol changes from changing contract WASM identities.
//!
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

/// Themes are separate secrets for each full player public key.
pub const THEME_KEY_PREFIX: &[u8] = b"freechess:theme:v1:";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PieceSet {
    #[default]
    Neo,
    Classic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    pub light_square: String,
    pub dark_square: String,
    pub piece_set: PieceSet,
    pub white_piece: String,
    pub black_piece: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            light_square: "#ebecd0".into(),
            dark_square: "#739552".into(),
            piece_set: PieceSet::Neo,
            white_piece: "#f9f9f9".into(),
            black_piece: "#555555".into(),
        }
    }
}

impl Theme {
    pub fn valid(&self) -> bool {
        [
            &self.light_square,
            &self.dark_square,
            &self.white_piece,
            &self.black_piece,
        ]
        .into_iter()
        .all(|color| {
            color.len() == 7
                && color.starts_with('#')
                && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        })
    }
}

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
pub const LEGACY_DELEGATE_CODE_HASHES: &[[u8; 32]] = &[
    [
        0x58, 0x84, 0xcd, 0x51, 0x7d, 0x11, 0xe5, 0x7b, 0x68, 0x05, 0x2a, 0xd3, 0xa5, 0xdd, 0xc2,
        0xf1, 0xb1, 0x51, 0x82, 0x93, 0x3a, 0xad, 0x30, 0x08, 0xf3, 0xbf, 0xa6, 0x99, 0x4e, 0xab,
        0xcf, 0x96,
    ],
    [
        0x88, 0xfe, 0x5a, 0x45, 0x4b, 0xdf, 0x99, 0xad, 0x29, 0x5c, 0xb6, 0xdf, 0x7f, 0xe1, 0xf4,
        0x58, 0x18, 0x27, 0x5d, 0xad, 0xe2, 0x65, 0x1d, 0x96, 0x28, 0x3a, 0x53, 0x70, 0x0c, 0xdd,
        0x9e, 0xc6,
    ],
    [
        0xc1, 0x70, 0x4d, 0x6b, 0xdc, 0xdb, 0x50, 0x78, 0x4b, 0xdc, 0xed, 0xf6, 0x28, 0x4e, 0xb3,
        0x13, 0x63, 0xb7, 0x9e, 0x85, 0x5e, 0x5d, 0xe9, 0xd8, 0xcc, 0xb4, 0x69, 0x75, 0xc8, 0x1e,
        0x7e, 0x3a,
    ],
];

/// Retired delegates that support per-player themes, newest first.
pub const LEGACY_THEME_DELEGATE_CODE_HASHES: &[[u8; 32]] = &[[
    0x58, 0x84, 0xcd, 0x51, 0x7d, 0x11, 0xe5, 0x7b, 0x68, 0x05, 0x2a, 0xd3, 0xa5, 0xdd, 0xc2, 0xf1,
    0xb1, 0x51, 0x82, 0x93, 0x3a, 0xad, 0x30, 0x08, 0xf3, 0xbf, 0xa6, 0x99, 0x4e, 0xab, 0xcf, 0x96,
]];

/// The code hash the current delegate build is expected to have.
///
/// Its only job is to make an unnoticed change loud: `check-delegate-migration`
/// compares the built WASM against it, so re-keying the delegate becomes a
/// build failure rather than a silent loss of every account.
pub const CURRENT_DELEGATE_CODE_HASH: [u8; 32] = [
    0x4c, 0xca, 0x39, 0xd7, 0xe5, 0x05, 0x0c, 0xe0, 0xb6, 0x6c, 0x03, 0x5c, 0xb0, 0xb4, 0xa9, 0x3d,
    0x22, 0x2f, 0x5e, 0x16, 0x18, 0xe0, 0x20, 0x14, 0xb6, 0x5b, 0x27, 0x4b, 0x32, 0xcb, 0x30, 0x59,
];

/// The delegate key a given code hash produces, mirroring freenet-stdlib's
/// `generate_id`.
///
/// FreeChess registers its delegate with empty parameters, so the derivation
/// collapses to a second BLAKE3 over the code hash. The UI's account recovery
/// and the protocol tests share this implementation.
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
    GetTheme {
        player: [u8; 32],
    },
    SetTheme {
        player: [u8; 32],
        theme: Theme,
    },
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
    /// Read result for this player's stored theme.
    Theme {
        player: [u8; 32],
        theme: Option<Theme>,
    },
    /// Acknowledgment of the exact theme successfully saved for this player.
    ThemeSaved {
        player: [u8; 32],
        theme: Theme,
    },
    /// Confirms which seed was saved without relying on host message context.
    AccountStored {
        seed: [u8; 32],
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

    /// Our derivation must agree with the one freenet-stdlib actually uses to
    /// key a delegate. If it ever drifts, account recovery would probe keys
    /// that never existed and every migration would silently recover nothing —
    /// so pin it against the real implementation rather than against a constant
    /// we wrote ourselves.
    #[test]
    fn the_delegate_key_derivation_matches_the_stdlib() {
        use freenet_stdlib::prelude::{Delegate, DelegateCode, Parameters};

        // Any bytes will do: what is under test is the hashing, not the module.
        let wasm = b"not really wasm, but hashed the same way".to_vec();
        let code = DelegateCode::from(wasm.clone());
        let delegate = Delegate::from((&code, &Parameters::from(vec![])));

        let code_hash: [u8; 32] = *blake3::hash(&wasm).as_bytes();
        assert_eq!(
            code_hash.as_slice(),
            delegate.key().code_hash().as_ref(),
            "code hash is BLAKE3 over the module bytes"
        );
        assert_eq!(
            delegate_key_for(&code_hash).as_slice(),
            delegate.key().bytes(),
            "with empty parameters the key is BLAKE3 over the code hash"
        );
    }

    /// A retired delegate must never be listed as the current one — that would
    /// mean the client probes the key it is already using and recovers nothing.
    #[test]
    fn the_current_delegate_is_not_also_listed_as_legacy() {
        assert!(
            !LEGACY_DELEGATE_CODE_HASHES.contains(&CURRENT_DELEGATE_CODE_HASH),
            "the current delegate is also listed as retired"
        );
        let mut seen = LEGACY_DELEGATE_CODE_HASHES.to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "duplicate entries in the legacy list");
    }

    #[test]
    fn retired_theme_delegates_remain_available_for_account_recovery() {
        assert!(LEGACY_THEME_DELEGATE_CODE_HASHES
            .iter()
            .all(|hash| LEGACY_DELEGATE_CODE_HASHES.contains(hash)));
    }

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
            ChessDelegateRequest::GetTheme { player: [1; 32] },
            ChessDelegateRequest::SetTheme {
                player: [2; 32],
                theme: Theme {
                    piece_set: PieceSet::Classic,
                    ..Theme::default()
                },
            },
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
            ChessDelegateResponse::Theme {
                player: [1; 32],
                theme: None,
            },
            ChessDelegateResponse::Theme {
                player: [2; 32],
                theme: Some(Theme::default()),
            },
            ChessDelegateResponse::ThemeSaved {
                player: [2; 32],
                theme: Theme::default(),
            },
            ChessDelegateResponse::AccountStored { seed: [7; 32] },
        ] {
            let bytes = resp.to_bytes().expect("encodes");
            assert_eq!(ChessDelegateResponse::from_bytes(&bytes).unwrap(), resp);
        }
    }

    #[test]
    fn a_truncated_payload_is_an_error_not_a_panic() {
        assert!(ChessDelegateRequest::from_bytes(&[0xff, 0x01]).is_err());
    }

    #[test]
    fn theme_colors_are_exactly_six_hex_digits() {
        assert!(Theme::default().valid());
        for index in 0..4 {
            for invalid in [
                "red",
                "#abc",
                "#12345g",
                "1234567",
                "#123456; background:url(x)",
                "#abcéé",
            ] {
                let mut theme = Theme::default();
                let colors = [
                    &mut theme.light_square,
                    &mut theme.dark_square,
                    &mut theme.white_piece,
                    &mut theme.black_piece,
                ];
                *colors.into_iter().nth(index).unwrap() = invalid.into();
                assert!(!theme.valid(), "accepted invalid theme color {invalid:?}");
            }
        }
        assert!(Theme {
            white_piece: "#aAbBcC".into(),
            ..Theme::default()
        }
        .valid());
    }

    #[test]
    fn adding_theme_messages_preserves_existing_wire_tags() {
        assert_eq!(
            ChessDelegateRequest::GetAccount.to_bytes().unwrap(),
            b"\x6aGetAccount"
        );
        assert_eq!(
            ChessDelegateResponse::Stored.to_bytes().unwrap(),
            b"\x66Stored"
        );
    }
}
