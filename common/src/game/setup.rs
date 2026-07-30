//! The creator-signed opening terms of a game, and the proof-of-work that
//! makes creating one cost something.
//!
//! # Why proof-of-work
//!
//! The Freenet whitepaper is explicit that the platform does not solve Sybil
//! resistance at the routing layer, and that defenses belong to the
//! application. A game is a whole contract instance, so an attacker who can
//! mint them for free can flood the lobby with junk from an unlimited supply of
//! fresh keys — no per-key quota helps, because keys are free.
//!
//! Binding the `game_id` to `BLAKE3(creator ‖ created_at ‖ nonce)` with a
//! required number of leading zero bits makes each game cost measurable work,
//! and because `game_id` is a *contract parameter* the cost is bound to the
//! contract key itself. Any peer re-verifies it from the bytes alone, with no
//! shared counter and no coordination.

use super::{ChessGameParametersV1, ChessGameStateV1, SIG_DOMAIN};
use crate::chess::Color;
use crate::identity::{verify_sig, GameId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};

/// Leading zero bits required on a `game_id`.
///
/// 16 bits is ~65k hashes — a few milliseconds for an honest client creating
/// one game, but it puts a hard floor under the cost of flooding the lobby with
/// thousands. Raising this is a WASM change and therefore a contract re-key, so
/// it must go through the migration registry.
pub const POW_DIFFICULTY_BITS: u32 = 16;

/// Bounds on the time control, rejecting nonsense values that would make the
/// clock display meaningless.
pub const MIN_INITIAL_SECS: u32 = 10;
pub const MAX_INITIAL_SECS: u32 = 24 * 60 * 60;
pub const MAX_INCREMENT_SECS: u32 = 300;

/// Longest nickname the creator may advertise on the game.
pub const MAX_NICKNAME_LEN: usize = 32;

/// Clock settings for a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeControl {
    /// Starting time per player, in seconds.
    pub initial_secs: u32,
    /// Seconds added after each move (Fischer increment).
    pub increment_secs: u32,
}

impl Default for TimeControl {
    fn default() -> Self {
        // 10+0 rapid: the most common casual control.
        TimeControl {
            initial_secs: 600,
            increment_secs: 0,
        }
    }
}

impl TimeControl {
    pub fn verify(&self) -> Result<(), String> {
        if !(MIN_INITIAL_SECS..=MAX_INITIAL_SECS).contains(&self.initial_secs) {
            return Err(format!(
                "initial time {}s outside {}..={}",
                self.initial_secs, MIN_INITIAL_SECS, MAX_INITIAL_SECS
            ));
        }
        if self.increment_secs > MAX_INCREMENT_SECS {
            return Err(format!(
                "increment {}s exceeds {}",
                self.increment_secs, MAX_INCREMENT_SECS
            ));
        }
        Ok(())
    }

    /// Short label such as `10+5`, as chess sites display it.
    pub fn label(&self) -> String {
        format!("{}+{}", self.initial_secs / 60, self.increment_secs)
    }

    /// Category used for the lobby filters, following the usual thresholds on
    /// estimated total game duration.
    pub fn category(&self) -> &'static str {
        let estimated = self.initial_secs + 40 * self.increment_secs;
        if estimated < 180 {
            "Bullet"
        } else if estimated < 600 {
            "Blitz"
        } else if estimated < 1800 {
            "Rapid"
        } else {
            "Classical"
        }
    }
}

/// The creator's opening terms. Every field is covered by the signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSetup {
    /// Which color the creator takes; the challenger gets the other.
    pub creator_plays: Color,
    pub time_control: TimeControl,
    /// Unix milliseconds when the game was created.
    pub created_at: i64,
    /// Proof-of-work nonce; see [`GameSetup::derive_game_id`].
    pub pow_nonce: u64,
    /// Display name the creator advertises. The authoritative nickname lives in
    /// the player contract; this copy lets the lobby render a game without
    /// fetching a second contract.
    pub creator_nickname: String,
    /// A direct challenge to one specific player — for inviting someone you
    /// spotted spectating a game. When set, only this key may take the seat
    /// (enforced in [`crate::game::opponent::OpponentSlotV1::verify`]); when
    /// `None` the game is open to anyone.
    #[serde(default, with = "opt_key_serde")]
    pub challenged: Option<VerifyingKey>,
}

/// Serde for the optional challenged-player key.
mod opt_key_serde {
    use ed25519_dalek::VerifyingKey;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(key: &Option<VerifyingKey>, s: S) -> Result<S::Ok, S::Error> {
        match key {
            Some(k) => Some(k.to_bytes().to_vec()).serialize(s),
            None => None::<Vec<u8>>.serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<VerifyingKey>, D::Error> {
        let bytes = Option::<Vec<u8>>::deserialize(d)?;
        match bytes {
            None => Ok(None),
            Some(b) => {
                let arr: [u8; 32] = b
                    .as_slice()
                    .try_into()
                    .map_err(|_| serde::de::Error::custom("key must be 32 bytes"))?;
                Ok(Some(
                    VerifyingKey::from_bytes(&arr).map_err(serde::de::Error::custom)?,
                ))
            }
        }
    }
}

impl GameSetup {
    /// The `game_id` implied by these terms. This *is* the proof of work: the
    /// creator searches `pow_nonce` until the digest has enough leading zeros.
    pub fn derive_game_id(&self, creator: &VerifyingKey) -> GameId {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SIG_DOMAIN);
        hasher.update(b"gameid:");
        hasher.update(creator.as_bytes());
        hasher.update(&self.created_at.to_le_bytes());
        hasher.update(&self.pow_nonce.to_le_bytes());
        GameId(*hasher.finalize().as_bytes())
    }

    /// Bytes covered by the creator's signature.
    fn signing_bytes(&self, game_id: &GameId) -> Vec<u8> {
        let mut buf = Vec::with_capacity(96);
        buf.extend_from_slice(SIG_DOMAIN);
        buf.extend_from_slice(b"setup:");
        buf.extend_from_slice(&game_id.0);
        buf.push(match self.creator_plays {
            Color::White => 0,
            Color::Black => 1,
        });
        buf.extend_from_slice(&self.time_control.initial_secs.to_le_bytes());
        buf.extend_from_slice(&self.time_control.increment_secs.to_le_bytes());
        buf.extend_from_slice(&self.created_at.to_le_bytes());
        buf.extend_from_slice(&self.pow_nonce.to_le_bytes());
        // Length-prefixed so a nickname cannot be shifted into an adjacent
        // field to forge a different setup under the same signature.
        buf.extend_from_slice(&(self.creator_nickname.len() as u32).to_le_bytes());
        buf.extend_from_slice(self.creator_nickname.as_bytes());
        // Covered by the signature, so nobody can strip the restriction off a
        // direct challenge and walk into someone else's invitation.
        match &self.challenged {
            Some(key) => {
                buf.push(1);
                buf.extend_from_slice(key.as_bytes());
            }
            None => buf.push(0),
        }
        buf
    }

    /// Search for a nonce satisfying the difficulty. Runs on the creating
    /// client, not in the contract.
    pub fn mine(
        creator: &VerifyingKey,
        creator_plays: Color,
        time_control: TimeControl,
        created_at: i64,
        creator_nickname: String,
    ) -> GameSetup {
        Self::mine_challenge(
            creator,
            creator_plays,
            time_control,
            created_at,
            creator_nickname,
            None,
        )
    }

    /// As [`mine`](Self::mine), but restricted to a single invited opponent.
    pub fn mine_challenge(
        creator: &VerifyingKey,
        creator_plays: Color,
        time_control: TimeControl,
        created_at: i64,
        creator_nickname: String,
        challenged: Option<VerifyingKey>,
    ) -> GameSetup {
        let mut setup = GameSetup {
            creator_plays,
            time_control,
            created_at,
            pow_nonce: 0,
            creator_nickname,
            challenged,
        };
        loop {
            let id = setup.derive_game_id(creator);
            if leading_zero_bits(&id.0) >= POW_DIFFICULTY_BITS {
                return setup;
            }
            setup.pow_nonce = setup.pow_nonce.wrapping_add(1);
        }
    }

    /// Sign these terms, producing the value that goes into contract state.
    pub fn sign(self, key: &SigningKey, game_id: &GameId) -> SignedGameSetup {
        let signature = key.sign(&self.signing_bytes(game_id));
        SignedGameSetup {
            setup: self,
            creator: key.verifying_key(),
            signature,
        }
    }
}

/// Number of leading zero bits in a digest.
pub fn leading_zero_bits(bytes: &[u8]) -> u32 {
    let mut count = 0;
    for byte in bytes {
        let zeros = byte.leading_zeros();
        count += zeros;
        if zeros != 8 {
            break;
        }
    }
    count
}

/// The creator's terms plus their signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedGameSetup {
    pub setup: GameSetup,
    /// Repeated here (it is also in parameters) so the struct is
    /// self-contained for signature checking.
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub creator: VerifyingKey,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl SignedGameSetup {
    pub fn creator_key(&self) -> VerifyingKey {
        self.creator
    }

    /// Check every invariant on the bytes alone: signature, proof-of-work, the
    /// binding to the contract parameters, and field bounds.
    pub fn verify(&self, params: &ChessGameParametersV1) -> Result<(), String> {
        // The creator embedded in state must be the one in parameters,
        // otherwise a peer could swap in its own key and re-sign everything.
        if self.creator != params.creator {
            return Err("setup creator does not match contract parameters".to_string());
        }
        // The game_id must be the proof-of-work digest of these exact terms.
        let derived = self.setup.derive_game_id(&self.creator);
        if derived != params.game_id {
            return Err("game_id is not the proof-of-work digest of the setup".to_string());
        }
        if leading_zero_bits(&params.game_id.0) < POW_DIFFICULTY_BITS {
            return Err(format!(
                "proof-of-work too weak: {} bits, need {}",
                leading_zero_bits(&params.game_id.0),
                POW_DIFFICULTY_BITS
            ));
        }
        self.setup.time_control.verify()?;
        if self.setup.creator_nickname.len() > MAX_NICKNAME_LEN {
            return Err(format!("nickname longer than {MAX_NICKNAME_LEN} bytes"));
        }
        if self.setup.created_at <= 0 {
            return Err("created_at must be positive".to_string());
        }
        // Challenging yourself would be a game with one participant.
        if self.setup.challenged == Some(self.creator) {
            return Err("the creator cannot challenge themselves".to_string());
        }
        verify_sig(
            &self.creator,
            &self.setup.signing_bytes(&params.game_id),
            &self.signature,
            "game setup",
        )
    }
}

/// The setup slot in game state.
///
/// Write-once: it is established by the initial PUT and there is no legitimate
/// update. Merge keeps whichever value is present, so it is trivially
/// commutative and idempotent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSetupV1(pub Option<SignedGameSetup>);

impl GameSetupV1 {
    pub fn get(&self) -> Option<&SignedGameSetup> {
        self.0.as_ref()
    }
}

impl ComposableState for GameSetupV1 {
    type ParentState = ChessGameStateV1;
    type Summary = bool;
    type Delta = SignedGameSetup;
    type Parameters = ChessGameParametersV1;

    fn verify(&self, _parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        match &self.0 {
            Some(setup) => setup.verify(params),
            // An empty state is valid — it is what a freshly created contract
            // looks like before the initial PUT lands.
            None => Ok(()),
        }
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        self.0.is_some()
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        // Only ship the setup to a peer that does not have it yet.
        if *old_summary {
            None
        } else {
            self.0.clone()
        }
    }

    fn apply_delta(
        &mut self,
        _parent: &Self::ParentState,
        params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(incoming) = delta {
            incoming.verify(params)?;
            // Write-once. A verified setup is uniquely determined by the
            // parameters (the game_id is its own hash), so a second valid one
            // cannot differ — ignoring it keeps apply idempotent.
            if self.0.is_none() {
                self.0 = Some(incoming.clone());
            }
        }
        Ok(())
    }
}
