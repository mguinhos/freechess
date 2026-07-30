//! The second player's slot — the mechanism that closes a game to exactly two
//! participants.
//!
//! # The join race
//!
//! Two challengers can click "play" at the same instant on different peers.
//! Both joins are validly signed, and neither peer knows about the other. The
//! merge cannot pick "the first one to arrive", because arrival order differs
//! per peer and that would break both commutativity and idempotence — the
//! whitepaper is explicit that eviction must be a deterministic function of the
//! merged set, not of arrival order.
//!
//! So the winner is chosen by a total order over the joins themselves:
//! earliest `joined_at`, then lowest key bytes as the tiebreak. Every peer
//! computes the same winner from the same set, in any order, however many times
//! it merges. Once converged, the slot never changes again — which is what
//! makes "nobody else can move in this game" hold.

use super::{ChessGameParametersV1, ChessGameStateV1, SIG_DOMAIN};
use crate::identity::{verify_sig, GameId, PlayerId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};

use super::setup::MAX_NICKNAME_LEN;

/// A challenger's signed claim on the open seat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedJoin {
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub player: VerifyingKey,
    /// Unix milliseconds the challenger claims to have joined at.
    pub joined_at: i64,
    /// Display name, same role as the creator's copy in the setup.
    pub nickname: String,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl SignedJoin {
    fn signing_bytes(
        game_id: &GameId,
        player: &VerifyingKey,
        joined_at: i64,
        nickname: &str,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(96);
        buf.extend_from_slice(SIG_DOMAIN);
        buf.extend_from_slice(b"join:");
        buf.extend_from_slice(&game_id.0);
        buf.extend_from_slice(player.as_bytes());
        buf.extend_from_slice(&joined_at.to_le_bytes());
        buf.extend_from_slice(&(nickname.len() as u32).to_le_bytes());
        buf.extend_from_slice(nickname.as_bytes());
        buf
    }

    /// Sign a claim on the open seat of `game_id`.
    pub fn new(key: &SigningKey, game_id: &GameId, joined_at: i64, nickname: String) -> SignedJoin {
        let player = key.verifying_key();
        let signature = key.sign(&Self::signing_bytes(game_id, &player, joined_at, &nickname));
        SignedJoin {
            player,
            joined_at,
            nickname,
            signature,
        }
    }

    pub fn player_id(&self) -> PlayerId {
        PlayerId::from(&self.player)
    }

    pub fn verify(&self, params: &ChessGameParametersV1) -> Result<(), String> {
        // A player cannot face themselves — that would let one key control both
        // sides and manufacture rated results at will.
        if self.player == params.creator {
            return Err("the creator cannot join their own game".to_string());
        }
        if self.nickname.len() > MAX_NICKNAME_LEN {
            return Err(format!("nickname longer than {MAX_NICKNAME_LEN} bytes"));
        }
        if self.joined_at <= 0 {
            return Err("joined_at must be positive".to_string());
        }
        verify_sig(
            &self.player,
            &Self::signing_bytes(
                &params.game_id,
                &self.player,
                self.joined_at,
                &self.nickname,
            ),
            &self.signature,
            "join",
        )
    }

    /// Total order used to settle a concurrent join race. Deterministic and
    /// independent of arrival order.
    fn race_key(&self) -> (i64, [u8; 32]) {
        (self.joined_at, self.player.to_bytes())
    }

    /// The winner of a race between two valid joins.
    fn wins_over(&self, other: &SignedJoin) -> bool {
        self.race_key() < other.race_key()
    }
}

/// The opponent seat: empty, or permanently filled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpponentSlotV1(pub Option<SignedJoin>);

impl OpponentSlotV1 {
    pub fn get(&self) -> Option<&SignedJoin> {
        self.0.as_ref()
    }

    /// Merge in a join, keeping the deterministic race winner.
    ///
    /// Idempotent (merging the same join twice is a no-op), commutative and
    /// associative, because it is a `min` over a total order.
    fn absorb(&mut self, incoming: SignedJoin) {
        match &self.0 {
            None => self.0 = Some(incoming),
            Some(current) => {
                if incoming.wins_over(current) {
                    self.0 = Some(incoming);
                }
            }
        }
    }
}

impl ComposableState for OpponentSlotV1 {
    type ParentState = ChessGameStateV1;
    type Summary = Option<[u8; 32]>;
    type Delta = SignedJoin;
    type Parameters = ChessGameParametersV1;

    fn verify(&self, parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        match &self.0 {
            Some(join) => {
                join.verify(params)?;
                // A direct challenge names its opponent, so nobody else may
                // take that seat — this is what makes "invite that spectator"
                // an invitation rather than an open game.
                if let Some(setup) = parent.setup.get() {
                    if let Some(invited) = setup.setup.challenged {
                        if join.player != invited {
                            return Err(
                                "this game is a direct challenge to a different player".to_string()
                            );
                        }
                    }
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        self.0.as_ref().map(|j| j.player.to_bytes())
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let current = self.0.as_ref()?;
        // Ship our join if the peer has none, or has a different one — they may
        // be holding the loser of a race and need ours to converge.
        match old_summary {
            Some(theirs) if theirs == &current.player.to_bytes() => None,
            _ => Some(current.clone()),
        }
    }

    fn apply_delta(
        &mut self,
        parent: &Self::ParentState,
        params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(join) = delta {
            join.verify(params)?;
            if let Some(setup) = parent.setup.get() {
                if let Some(invited) = setup.setup.challenged {
                    if join.player != invited {
                        return Err(
                            "this game is a direct challenge to a different player".to_string()
                        );
                    }
                }
            }
            self.absorb(join.clone());
        }
        Ok(())
    }
}
