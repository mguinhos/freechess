//! Ways a game ends off the board: resignation, agreed draw, and timeout.
//!
//! Each is authorized by the party who can legitimately claim it, and only
//! within this one game — every signature covers the `game_id`, so a
//! resignation signed in one game is meaningless in any other, even between the
//! same two players.

use super::{
    ChessGameParametersV1, ChessGameStateV1, DrawReason, GameResult, WinReason, SIG_DOMAIN,
};
use crate::chess::Color;
use crate::identity::{verify_sig, GameId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};

/// Grace period added to a timeout claim, absorbing clock skew between peers
/// and network delay before a claim is considered provable.
pub const TIMEOUT_GRACE_MS: i64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConclusionKind {
    /// The signer gives up. Signed by the losing player themselves.
    Resignation,
    /// Both players agreed to a draw; carries the opponent's countersignature.
    DrawAgreement,
    /// The signer claims their opponent's clock ran out.
    TimeoutClaim,
}

/// A signed statement that the game is over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedConclusion {
    pub kind: ConclusionKind,
    /// The player making the claim.
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub claimant: VerifyingKey,
    /// Ply count the game had reached; binds the claim to a point in the game
    /// so an old signed draw offer cannot be replayed later.
    pub at_ply: u32,
    /// Unix milliseconds.
    pub at: i64,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
    /// For [`ConclusionKind::DrawAgreement`], the opponent's signature over the
    /// same bytes. `None` for the other kinds.
    #[serde(default, with = "opt_signature_serde")]
    pub countersignature: Option<Signature>,
    /// The countersigning opponent's key, present only for a draw agreement.
    #[serde(default, with = "opt_key_serde")]
    pub counterparty: Option<VerifyingKey>,
}

/// Serde for the optional signature/key fields.
mod opt_signature_serde {
    use ed25519_dalek::Signature;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(sig: &Option<Signature>, s: S) -> Result<S::Ok, S::Error> {
        match sig {
            Some(sig) => Some(sig.to_bytes().to_vec()).serialize(s),
            None => None::<Vec<u8>>.serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Signature>, D::Error> {
        let bytes = Option::<Vec<u8>>::deserialize(d)?;
        match bytes {
            None => Ok(None),
            Some(b) => {
                let arr: [u8; 64] = b
                    .as_slice()
                    .try_into()
                    .map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))?;
                Ok(Some(Signature::from_bytes(&arr)))
            }
        }
    }
}

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

fn conclusion_signing_bytes(
    game_id: &GameId,
    kind: ConclusionKind,
    at_ply: u32,
    at: i64,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(SIG_DOMAIN);
    buf.extend_from_slice(b"conclusion:");
    buf.extend_from_slice(&game_id.0);
    buf.push(match kind {
        ConclusionKind::Resignation => 0,
        ConclusionKind::DrawAgreement => 1,
        ConclusionKind::TimeoutClaim => 2,
    });
    buf.extend_from_slice(&at_ply.to_le_bytes());
    buf.extend_from_slice(&at.to_le_bytes());
    buf
}

impl SignedConclusion {
    /// Resign the game. Only ever costs the signer.
    pub fn resign(key: &SigningKey, game_id: &GameId, at_ply: u32, at: i64) -> SignedConclusion {
        let bytes = conclusion_signing_bytes(game_id, ConclusionKind::Resignation, at_ply, at);
        SignedConclusion {
            kind: ConclusionKind::Resignation,
            claimant: key.verifying_key(),
            at_ply,
            at,
            signature: key.sign(&bytes),
            countersignature: None,
            counterparty: None,
        }
    }

    /// A draw both players signed. Requires both keys, which is what stops one
    /// player from unilaterally declaring a draw.
    pub fn draw_agreement(
        proposer: &SigningKey,
        accepter: &SigningKey,
        game_id: &GameId,
        at_ply: u32,
        at: i64,
    ) -> SignedConclusion {
        let bytes = conclusion_signing_bytes(game_id, ConclusionKind::DrawAgreement, at_ply, at);
        SignedConclusion {
            kind: ConclusionKind::DrawAgreement,
            claimant: proposer.verifying_key(),
            at_ply,
            at,
            signature: proposer.sign(&bytes),
            countersignature: Some(accepter.sign(&bytes)),
            counterparty: Some(accepter.verifying_key()),
        }
    }

    /// Claim the opponent's clock expired. Verified against the move
    /// timestamps, so it cannot be claimed early.
    pub fn claim_timeout(
        key: &SigningKey,
        game_id: &GameId,
        at_ply: u32,
        at: i64,
    ) -> SignedConclusion {
        let bytes = conclusion_signing_bytes(game_id, ConclusionKind::TimeoutClaim, at_ply, at);
        SignedConclusion {
            kind: ConclusionKind::TimeoutClaim,
            claimant: key.verifying_key(),
            at_ply,
            at,
            signature: key.sign(&bytes),
            countersignature: None,
            counterparty: None,
        }
    }

    fn signing_bytes(&self, game_id: &GameId) -> Vec<u8> {
        conclusion_signing_bytes(game_id, self.kind, self.at_ply, self.at)
    }

    /// Deterministic total order for settling concurrent conclusions.
    fn tiebreak(&self) -> (i64, [u8; 64]) {
        (self.at, self.signature.to_bytes())
    }

    /// Validate against the game it claims to conclude.
    pub fn verify(
        &self,
        parent: &ChessGameStateV1,
        params: &ChessGameParametersV1,
    ) -> Result<(), String> {
        let (white, black) = parent
            .player_keys()
            .ok_or_else(|| "a game with no opponent cannot be concluded".to_string())?;

        // The claimant must be one of this game's two players — not merely
        // "some valid key". This is what keeps a third party from resigning
        // someone else's game.
        if self.claimant != white && self.claimant != black {
            return Err(
                "conclusion signed by someone who is not a player in this game".to_string(),
            );
        }
        if self.at <= 0 {
            return Err("conclusion timestamp must be positive".to_string());
        }
        verify_sig(
            &self.claimant,
            &self.signing_bytes(&params.game_id),
            &self.signature,
            "conclusion",
        )?;

        match self.kind {
            ConclusionKind::Resignation => Ok(()),

            ConclusionKind::DrawAgreement => {
                let counter_sig = self
                    .countersignature
                    .ok_or_else(|| "draw agreement lacks a countersignature".to_string())?;
                let counter_key = self
                    .counterparty
                    .ok_or_else(|| "draw agreement lacks a counterparty key".to_string())?;
                // The countersigner must be the *other* player: two signatures
                // from the same key are not an agreement.
                let expected_counter = if self.claimant == white { black } else { white };
                if counter_key != expected_counter {
                    return Err("draw countersigned by the wrong key".to_string());
                }
                verify_sig(
                    &counter_key,
                    &self.signing_bytes(&params.game_id),
                    &counter_sig,
                    "draw countersignature",
                )
            }

            ConclusionKind::TimeoutClaim => {
                // A timeout is only valid if the clocks say so. Everything the
                // check needs (time control, move timestamps) is in state, so
                // every peer reaches the same verdict.
                let loser = if self.claimant == white {
                    Color::Black
                } else {
                    Color::White
                };
                let setup = parent
                    .setup
                    .get()
                    .ok_or_else(|| "game has no setup".to_string())?;
                let elapsed = parent.time_used_by(loser);
                let budget_ms = setup.setup.time_control.initial_secs as i64 * 1000
                    + setup.setup.time_control.increment_secs as i64
                        * 1000
                        * parent.moves_made_by(loser) as i64;

                // Time the loser has burned since their opponent's last move,
                // measured up to the moment of the claim.
                let pending = parent.pending_time_for(loser, self.at);
                if elapsed + pending <= budget_ms + TIMEOUT_GRACE_MS {
                    return Err(format!(
                        "timeout claimed too early: {}ms used of {}ms budget",
                        elapsed + pending,
                        budget_ms
                    ));
                }
                Ok(())
            }
        }
    }

    /// The result this conclusion implies.
    pub fn result(&self, parent: &ChessGameStateV1) -> Option<GameResult> {
        let color = parent.color_of(&self.claimant)?;
        Some(match self.kind {
            // Resigning hands the win to the *other* side.
            ConclusionKind::Resignation => match color {
                Color::White => GameResult::BlackWins(WinReason::Resignation),
                Color::Black => GameResult::WhiteWins(WinReason::Resignation),
            },
            ConclusionKind::DrawAgreement => GameResult::Draw(DrawReason::Agreement),
            // A timeout claim is made by the winner.
            ConclusionKind::TimeoutClaim => match color {
                Color::White => GameResult::WhiteWins(WinReason::Timeout),
                Color::Black => GameResult::BlackWins(WinReason::Timeout),
            },
        })
    }
}

/// The conclusion slot: empty while the game is live.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConclusionV1(pub Option<SignedConclusion>);

impl ConclusionV1 {
    pub fn get(&self) -> Option<&SignedConclusion> {
        self.0.as_ref()
    }

    /// The result implied by the stored conclusion, if any. Needs the parent to
    /// map the claimant's key to a color.
    pub(super) fn result_with(&self, parent: &ChessGameStateV1) -> Option<GameResult> {
        self.0.as_ref()?.result(parent)
    }

    fn absorb(&mut self, incoming: SignedConclusion) {
        match &self.0 {
            None => self.0 = Some(incoming),
            Some(current) => {
                // Earliest claim wins, signature bytes break ties: a total
                // order, so merging is commutative and idempotent.
                if incoming.tiebreak() < current.tiebreak() {
                    self.0 = Some(incoming);
                }
            }
        }
    }
}

impl ComposableState for ConclusionV1 {
    type ParentState = ChessGameStateV1;
    type Summary = bool;
    type Delta = SignedConclusion;
    type Parameters = ChessGameParametersV1;

    fn verify(&self, parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        match &self.0 {
            Some(c) => c.verify(parent, params),
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
        if *old_summary {
            None
        } else {
            self.0.clone()
        }
    }

    fn apply_delta(
        &mut self,
        parent: &Self::ParentState,
        params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(incoming) = delta {
            incoming.verify(parent, params)?;
            self.absorb(incoming.clone());
        }
        Ok(())
    }
}
