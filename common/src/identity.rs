//! Player identity and the signing helpers shared by every contract.
//!
//! A player is an ed25519 keypair. The *public* half is what appears in state;
//! the private half never leaves the user's device (it lives in the delegate).
//! That keypair is the one piece of data in the system that is deliberately not
//! shared between peers — everything else (games, lobby, profiles, Elo) is
//! public contract state replicated to every peer.
//!
//! A player's durable handle is [`PlayerId`], derived from the verifying key —
//! never from a contract key, which moves whenever the WASM changes. That is
//! what lets a contract upgrade carry state forward without breaking profile
//! links or game history.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Short, self-certifying player handle: the first 16 bytes of
/// `BLAKE3(verifying_key)`, shown to users in Base58.
///
/// Truncated to 16 bytes because it appears in URLs and on the board; 128 bits
/// is far past what a collision search on a nickname system needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlayerId(pub [u8; 16]);

impl From<&VerifyingKey> for PlayerId {
    fn from(key: &VerifyingKey) -> Self {
        let hash = blake3::hash(key.as_bytes());
        let mut id = [0u8; 16];
        id.copy_from_slice(&hash.as_bytes()[..16]);
        PlayerId(id)
    }
}

impl PlayerId {
    pub fn to_base58(self) -> String {
        bs58::encode(self.0).into_string()
    }

    pub fn from_base58(s: &str) -> Option<PlayerId> {
        let bytes = bs58::decode(s).into_vec().ok()?;
        if bytes.len() != 16 {
            return None;
        }
        let mut id = [0u8; 16];
        id.copy_from_slice(&bytes);
        Some(PlayerId(id))
    }
}

impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_base58())
    }
}

/// Identifier of a single game instance, and simultaneously the proof that
/// creating it cost work. See [`crate::lobby`] for how it throttles spam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GameId(pub [u8; 32]);

impl GameId {
    pub fn to_base58(self) -> String {
        bs58::encode(self.0).into_string()
    }

    pub fn from_base58(s: &str) -> Option<GameId> {
        let bytes = bs58::decode(s).into_vec().ok()?;
        if bytes.len() != 32 {
            return None;
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes);
        Some(GameId(id))
    }
}

impl fmt::Display for GameId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_base58())
    }
}

/// Verify `signature` over `message` for `key`, with a uniform error string.
pub fn verify_sig(
    key: &VerifyingKey,
    message: &[u8],
    signature: &Signature,
    what: &str,
) -> Result<(), String> {
    key.verify(message, signature)
        .map_err(|e| format!("invalid signature on {what}: {e}"))
}

/// Serde bridge for `ed25519_dalek::Signature`, which is not `Serialize` in the
/// no-default-features configuration the contracts build with.
pub mod signature_serde {
    use ed25519_dalek::Signature;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(sig: &Signature, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&sig.to_bytes())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Signature, D::Error> {
        let bytes = <serde_bytes_compat::ByteBuf>::deserialize(d)?;
        let arr: [u8; 64] = bytes
            .0
            .as_slice()
            .try_into()
            .map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))?;
        Ok(Signature::from_bytes(&arr))
    }

    /// Minimal `serde_bytes`-style helper so we don't take the dependency just
    /// for one field type.
    pub mod serde_bytes_compat {
        use serde::{Deserialize, Deserializer};

        pub struct ByteBuf(pub Vec<u8>);

        struct BytesVisitor;

        // Accepts both encodings a byte field can arrive in: a native byte
        // string (what ciborium emits) and a sequence of integers (what
        // JSON-ish formats produce), so state stays readable across both.
        impl<'de> serde::de::Visitor<'de> for BytesVisitor {
            type Value = Vec<u8>;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a byte array")
            }

            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
                Ok(v.to_vec())
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Vec<u8>, A::Error> {
                let mut out = Vec::new();
                while let Some(b) = seq.next_element::<u8>()? {
                    out.push(b);
                }
                Ok(out)
            }
        }

        impl<'de> Deserialize<'de> for ByteBuf {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                d.deserialize_bytes(BytesVisitor).map(ByteBuf)
            }
        }
    }
}

/// Serde bridge for `VerifyingKey`, stored as raw 32 bytes.
pub mod verifying_key_serde {
    use ed25519_dalek::VerifyingKey;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(key: &VerifyingKey, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(key.as_bytes())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<VerifyingKey, D::Error> {
        let bytes = super::signature_serde::serde_bytes_compat::ByteBuf::deserialize(d)?;
        let arr: [u8; 32] = bytes
            .0
            .as_slice()
            .try_into()
            .map_err(|_| serde::de::Error::custom("verifying key must be 32 bytes"))?;
        VerifyingKey::from_bytes(&arr).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    #[test]
    fn player_id_is_stable_and_base58_roundtrips() {
        let key = SigningKey::generate(&mut OsRng).verifying_key();
        let id = PlayerId::from(&key);
        assert_eq!(id, PlayerId::from(&key), "derivation must be deterministic");
        assert_eq!(PlayerId::from_base58(&id.to_base58()), Some(id));
    }

    #[test]
    fn distinct_keys_give_distinct_ids() {
        let a = PlayerId::from(&SigningKey::generate(&mut OsRng).verifying_key());
        let b = PlayerId::from(&SigningKey::generate(&mut OsRng).verifying_key());
        assert_ne!(a, b);
    }

    #[test]
    fn game_id_base58_roundtrips_and_rejects_wrong_length() {
        let id = GameId([7u8; 32]);
        assert_eq!(GameId::from_base58(&id.to_base58()), Some(id));
        assert_eq!(
            GameId::from_base58(&bs58::encode([1u8; 16]).into_string()),
            None
        );
        assert_eq!(GameId::from_base58("not base58 !!"), None);
    }
}
