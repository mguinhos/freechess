//! Account import and export.
//!
//! A FreeChess account *is* an ed25519 signing key — there is no server to
//! hold a password against, and every game, rating and profile entry is bound
//! to that key. So moving an account between devices means moving the key.
//!
//! The exported form is a Base58 string with a version prefix and a checksum:
//!
//! ```text
//! fchess1<base58(seed[32] ‖ checksum[4])>
//! ```
//!
//! The checksum catches transcription mistakes before they turn into "your
//! account is gone" — an ed25519 key accepts *any* 32 bytes as valid, so
//! without it a single mistyped character silently yields a different, empty
//! account rather than an error.
//!
//! # Handling
//!
//! This string is the whole account. Anyone holding it can play, resign and
//! rate games as that player forever; there is no revocation and no reset. The
//! UI must present it as a secret, and never write it anywhere the network can
//! see — it is precisely the one piece of data that stays out of contract
//! state.

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use crate::identity::PlayerId;

/// Prefix identifying the format and version.
pub const EXPORT_PREFIX: &str = "fchess1";

const CHECKSUM_LEN: usize = 4;
const SEED_LEN: usize = 32;

/// Domain separator so this checksum can never be confused with another hash
/// in the protocol.
const CHECKSUM_DOMAIN: &[u8] = b"freechess:account-export:v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountError {
    /// Missing or unknown format prefix.
    BadPrefix,
    /// Not valid Base58.
    NotBase58,
    /// Decoded to the wrong number of bytes.
    BadLength(usize),
    /// Checksum mismatch — almost always a typo.
    BadChecksum,
}

impl core::fmt::Display for AccountError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AccountError::BadPrefix => write!(
                f,
                "not a FreeChess account key (should start with '{EXPORT_PREFIX}')"
            ),
            AccountError::NotBase58 => write!(f, "account key contains invalid characters"),
            AccountError::BadLength(n) => {
                write!(f, "account key has the wrong length ({n} bytes)")
            }
            AccountError::BadChecksum => write!(
                f,
                "account key failed its checksum — it was probably mistyped"
            ),
        }
    }
}

fn checksum(seed: &[u8; SEED_LEN]) -> [u8; CHECKSUM_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CHECKSUM_DOMAIN);
    hasher.update(seed);
    let digest = hasher.finalize();
    let mut out = [0u8; CHECKSUM_LEN];
    out.copy_from_slice(&digest.as_bytes()[..CHECKSUM_LEN]);
    out
}

/// Render a signing key as the portable account string.
pub fn export(key: &SigningKey) -> String {
    let seed = key.to_bytes();
    let mut payload = Vec::with_capacity(SEED_LEN + CHECKSUM_LEN);
    payload.extend_from_slice(&seed);
    payload.extend_from_slice(&checksum(&seed));
    format!("{EXPORT_PREFIX}{}", bs58::encode(payload).into_string())
}

/// Parse an exported account string back into a signing key.
///
/// Whitespace around the string is ignored, since it usually arrives pasted.
pub fn import(text: &str) -> Result<SigningKey, AccountError> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix(EXPORT_PREFIX)
        .ok_or(AccountError::BadPrefix)?;

    let bytes = bs58::decode(body)
        .into_vec()
        .map_err(|_| AccountError::NotBase58)?;
    if bytes.len() != SEED_LEN + CHECKSUM_LEN {
        return Err(AccountError::BadLength(bytes.len()));
    }

    let mut seed = [0u8; SEED_LEN];
    seed.copy_from_slice(&bytes[..SEED_LEN]);
    if checksum(&seed) != bytes[SEED_LEN..] {
        return Err(AccountError::BadChecksum);
    }
    Ok(SigningKey::from_bytes(&seed))
}

/// The player id an exported account string maps to, without importing it —
/// lets the UI show "this will import player-abc123" before committing.
pub fn preview_id(text: &str) -> Result<PlayerId, AccountError> {
    let key = import(text)?;
    Ok(PlayerId::from(&key.verifying_key()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    #[test]
    fn export_import_roundtrips_to_the_same_account() {
        let original = key();
        let text = export(&original);
        let restored = import(&text).expect("must import");
        assert_eq!(original.to_bytes(), restored.to_bytes());
        assert_eq!(
            PlayerId::from(&original.verifying_key()),
            PlayerId::from(&restored.verifying_key()),
            "the imported account must be the same player"
        );
    }

    #[test]
    fn exported_keys_carry_the_version_prefix() {
        assert!(export(&key()).starts_with(EXPORT_PREFIX));
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        let original = key();
        let text = format!("  \n{}\t ", export(&original));
        assert_eq!(import(&text).unwrap().to_bytes(), original.to_bytes());
    }

    #[test]
    fn a_mistyped_character_is_caught_by_the_checksum() {
        // Without a checksum this would silently import a *different* account,
        // which is the failure mode worth protecting against.
        let text = export(&key());
        let mut chars: Vec<char> = text.chars().collect();
        let last = chars.len() - 1;
        // Swap the final character for a different valid Base58 one.
        chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
        let typo: String = chars.into_iter().collect();

        match import(&typo) {
            Err(AccountError::BadChecksum) | Err(AccountError::BadLength(_)) => {}
            other => panic!("expected the typo to be rejected, got {other:?}"),
        }
    }

    #[test]
    fn a_string_without_the_prefix_is_rejected() {
        let text = export(&key());
        let without = text.strip_prefix(EXPORT_PREFIX).unwrap();
        assert_eq!(import(without), Err(AccountError::BadPrefix));
    }

    #[test]
    fn garbage_is_rejected_rather_than_producing_an_account() {
        for bad in ["", "fchess1", "fchess1!!!!", "totally not a key"] {
            assert!(import(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn preview_matches_the_imported_account() {
        let original = key();
        let text = export(&original);
        assert_eq!(
            preview_id(&text).unwrap(),
            PlayerId::from(&original.verifying_key())
        );
    }

    #[test]
    fn two_accounts_export_to_different_strings() {
        assert_ne!(export(&key()), export(&key()));
    }
}
