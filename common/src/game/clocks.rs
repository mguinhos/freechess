//! How two peers agree on the passage of time in a contract that has no clock.
//!
//! # The problem
//!
//! Every other rule in this crate is decidable from the bytes in state. A
//! timeout is not. "Your clock ran out" is a claim about *now*, and a contract
//! has no now: it sees only signed timestamps that the signers chose. A peer
//! validating a claim stamped eleven days from today cannot tell whether that
//! moment has arrived, so a bare timestamp is worth nothing — whoever claims
//! first wins, at any ply, against any opponent.
//!
//! No amount of bounding the claim's own timestamp fixes that, because the
//! bound would have to come from evidence the claimant does not control, and the
//! claimant controls every field of their own claim.
//!
//! # The construction
//!
//! The only party who can testify that a moment has passed without lying in
//! their own favour is the player who *loses* by it. So both players
//! continuously sign the time they see, and a timeout is judged against the
//! loser's own attestations rather than the winner's assertion:
//!
//! * While a game is live, each player republishes a [`ClockAttestation`] every
//!   [`CLOCK_TICK_MS`] — a signature over `(game_id, at)`, meaning "I am here
//!   and my clock reads `at`".
//! * An attestation only ever moves its own signer's deadline *later*, and only
//!   that signer holds the key, so there is no incentive to forge one and no way
//!   to forge someone else's.
//! * A player whose attestations stop advancing is, from state's point of view,
//!   gone. After [`ABSENCE_FORFEIT_MS`] of no new evidence their opponent can
//!   claim the game.
//!
//! That gives two provable ways to lose on time, and
//! [`ChessGameStateV1::timeout_provable_at`](super::ChessGameStateV1::timeout_provable_at)
//! computes the instant each one matures:
//!
//! 1. **Flag fall.** The loser's own attestations reach the moment their clock
//!    hits zero. They were present and they ran out of time. Unforgeable: the
//!    claim cannot mature until the loser themselves signs their way up to it.
//! 2. **Absence.** The loser's attestations stopped while they still had time on
//!    the clock. The claim matures [`ABSENCE_FORFEIT_MS`] after their last
//!    signature.
//!
//! # What this does and does not guarantee
//!
//! Case 1 is airtight — the evidence is the loser's own signature, so nobody
//! else can manufacture it.
//!
//! Case 2 cannot be, and it is worth being precise about why. Since a peer
//! cannot tell the future from the past, a claimant can pre-sign the absence
//! deadline and submit it before that deadline actually arrives. The exposure is
//! bounded and self-correcting rather than absent: a claim is only ever dated a
//! fixed distance past the loser's last signature, so the loser's *next*
//! attestation pushes the deadline beyond the claim and strips its effect
//! permanently — attestations only advance, so an invalidated claim never
//! recovers. A player who is actually at the board therefore cannot be robbed
//! for longer than the gap between their own ticks, while a player who has
//! genuinely left produces no further evidence and correctly forfeits.
//!
//! Certificates are the other half of that backstop: they need both players'
//! signatures, so a claim that only briefly held cannot be cashed into a rating.
//!
//! # Convergence
//!
//! One slot per player, merged by keeping the later attestation (signature bytes
//! break an exact tie), which is commutative, associative and idempotent. Because
//! attested time only ever increases, every derived deadline is a pure function
//! of the merged set and never of arrival order.

use super::{ChessGameParametersV1, ChessGameStateV1, SIG_DOMAIN};
use crate::identity::{signature_digest, verify_sig, GameId, PlayerId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How often a live client republishes its attestation.
///
/// Also the width of the window in which a pre-signed absence claim can hold
/// against a player who is really there, since their next tick invalidates it.
pub const CLOCK_TICK_MS: i64 = 10_000;

/// How long a player's attestations may go stale before their opponent can
/// claim the game.
///
/// Four missed ticks, so an ordinary network hiccup or a browser throttling a
/// backgrounded tab does not cost anybody a game.
pub const ABSENCE_FORFEIT_MS: i64 = 45_000;

/// A player's signed statement that they are present and what time they see.
///
/// Deliberately carries nothing else. It is not a heartbeat for presence (that
/// lives in the lobby, which a game contract cannot read) and it says nothing
/// about the position — its only job is to be evidence, signed by the one party
/// it can count against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockAttestation {
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub player: VerifyingKey,
    /// Unix milliseconds, as the signer's own clock reads it.
    pub at: i64,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl ClockAttestation {
    fn signing_bytes(game_id: &GameId, at: i64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(SIG_DOMAIN);
        buf.extend_from_slice(b"clock:");
        buf.extend_from_slice(&game_id.0);
        buf.extend_from_slice(&at.to_le_bytes());
        buf
    }

    pub fn new(key: &SigningKey, game_id: &GameId, at: i64) -> ClockAttestation {
        ClockAttestation {
            player: key.verifying_key(),
            at,
            signature: key.sign(&Self::signing_bytes(game_id, at)),
        }
    }

    pub fn player_id(&self) -> PlayerId {
        PlayerId::from(&self.player)
    }

    /// Check the signature against the embedded key. Says the attestation is
    /// authentic, not that its author plays in this game — that is
    /// [`ClocksV1::check`]'s job, because it needs the opponent slot.
    pub fn verify_signature(&self, game_id: &GameId) -> Result<(), String> {
        if self.at <= 0 {
            return Err("clock attestation has a non-positive timestamp".to_string());
        }
        verify_sig(
            &self.player,
            &Self::signing_bytes(game_id, self.at),
            &self.signature,
            "clock attestation",
        )
    }

    /// Total order used to settle two attestations from one player. Later wins;
    /// signature bytes break an exact tie so every peer picks the same one.
    fn tiebreak(&self) -> (i64, [u8; 64]) {
        (self.at, self.signature.to_bytes())
    }
}

/// The latest attestation from each player.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClocksV1 {
    /// `BTreeMap` rather than `HashMap`: the summary derives from this and must
    /// serialize deterministically, or core's convergence check misfires.
    pub attestations: BTreeMap<PlayerId, ClockAttestation>,
}

impl ClocksV1 {
    /// The latest time `key` has attested to, if they ever have.
    pub fn attested_at(&self, key: &VerifyingKey) -> Option<i64> {
        self.attestations.get(&PlayerId::from(key)).map(|a| a.at)
    }

    /// Merge one attestation in, keeping whichever is later.
    fn absorb(&mut self, incoming: ClockAttestation) {
        let id = incoming.player_id();
        match self.attestations.get(&id) {
            Some(existing) if incoming.tiebreak() <= existing.tiebreak() => {}
            _ => {
                self.attestations.insert(id, incoming);
            }
        }
    }

    /// Drop attestations from anyone who is not one of the two players. Pure
    /// function of the merged contents, so every peer drops the same ones.
    pub(super) fn prune(&mut self, keys: Option<(VerifyingKey, VerifyingKey)>) {
        match keys {
            Some((white, black)) => {
                let (white, black) = (PlayerId::from(&white), PlayerId::from(&black));
                self.attestations
                    .retain(|id, _| *id == white || *id == black);
            }
            // No opponent yet, so there is no game to keep time for.
            None => self.attestations.clear(),
        }
    }

    /// Reject rather than repair, for the full-state PUT path that skips
    /// [`prune`](Self::prune).
    fn check(
        &self,
        game_id: &GameId,
        keys: Option<(VerifyingKey, VerifyingKey)>,
    ) -> Result<(), String> {
        if self.attestations.is_empty() {
            return Ok(());
        }
        let (white, black) = keys
            .ok_or_else(|| "clock attestations recorded before an opponent joined".to_string())?;
        for (id, attestation) in &self.attestations {
            if attestation.player != white && attestation.player != black {
                return Err("clock attestation from someone who is not a player".to_string());
            }
            if *id != attestation.player_id() {
                return Err("clock attestation is filed under the wrong player".to_string());
            }
            attestation.verify_signature(game_id)?;
        }
        Ok(())
    }
}

impl ComposableState for ClocksV1 {
    type ParentState = ChessGameStateV1;
    /// Per player, a fingerprint of the attestation we hold. The time alone is
    /// coarser than the merge order, which breaks a tie on it by signature
    /// bytes, so two attestations stamped the same millisecond would not have
    /// reconciled.
    type Summary = Vec<(PlayerId, u64)>;
    type Delta = Vec<ClockAttestation>;
    type Parameters = ChessGameParametersV1;

    fn verify(&self, parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        self.check(&params.game_id, parent.player_keys())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        self.attestations
            .iter()
            .map(|(id, a)| (*id, signature_digest(&a.signature)))
            .collect()
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let theirs: BTreeMap<PlayerId, u64> = old_summary.iter().copied().collect();
        let differing: Vec<ClockAttestation> = self
            .attestations
            .iter()
            .filter(|(id, a)| theirs.get(id) != Some(&signature_digest(&a.signature)))
            .map(|(_, a)| a.clone())
            .collect();
        if differing.is_empty() {
            None
        } else {
            Some(differing)
        }
    }

    fn apply_delta(
        &mut self,
        _parent: &Self::ParentState,
        params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(incoming) = delta {
            for attestation in incoming {
                // Only authenticity here. Whether the signer plays in this game
                // is settled by the prune pass once the whole state is merged,
                // because the opponent slot may be arriving in the same delta.
                attestation.verify_signature(&params.game_id)?;
                self.absorb(attestation.clone());
            }
        }
        Ok(())
    }
}
