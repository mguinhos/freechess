//! The signed move list.
//!
//! # Why nobody but the two players can move
//!
//! Each move carries the mover's public key and a signature over
//! `(game_id, ply, move, timestamp)`. Validation requires that key to be
//! *exactly* the key owning the side to move at that ply — White on even plies,
//! Black on odd — where the two keys come from the contract parameters (the
//! creator) and the sealed opponent slot.
//!
//! A spectator, a relaying peer, or anyone else has no private key satisfying
//! that check, so their move fails `verify` on every peer that receives it.
//! There is no "authorization list" to tamper with: the check is a signature
//! against a key fixed by the contract key itself.
//!
//! Playing out of turn is the same check. A player's key is only accepted on
//! their own plies, so signing a move for the opponent's ply fails exactly as a
//! stranger's would.
//!
//! # Convergence
//!
//! Moves merge as a union over a `BTreeMap` keyed by ply, which is commutative,
//! associative and idempotent. Two conflicting moves for the same ply (only
//! possible if a player double-signs) are settled by a total order on the
//! signature bytes, never by arrival order. The resulting map is then pruned to
//! its longest legal prefix by
//! [`ChessGameStateV1::prune`](super::ChessGameStateV1::prune) — a pure
//! function of the merged set, so every peer converges to the same move list.

use super::{ChessGameParametersV1, ChessGameStateV1, SIG_DOMAIN};
use crate::chess::{Game, Move};
use crate::identity::{verify_sig, GameId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A single move, signed by the player who made it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizedMove {
    /// Half-move number, starting at 0 for White's first move.
    pub ply: u32,
    pub mv: Move,
    /// Unix milliseconds, used to run the clocks and judge timeout claims.
    pub played_at: i64,
    /// The mover's key. Present so the signature is checkable in isolation;
    /// `verify` then requires it to match the side to move at this ply.
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub player: VerifyingKey,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl AuthorizedMove {
    fn signing_bytes(game_id: &GameId, ply: u32, mv: &Move, played_at: i64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(SIG_DOMAIN);
        buf.extend_from_slice(b"move:");
        buf.extend_from_slice(&game_id.0);
        buf.extend_from_slice(&ply.to_le_bytes());
        buf.extend_from_slice(&mv.signing_bytes());
        buf.extend_from_slice(&played_at.to_le_bytes());
        buf
    }

    pub fn new(
        key: &SigningKey,
        game_id: &GameId,
        ply: u32,
        mv: Move,
        played_at: i64,
    ) -> AuthorizedMove {
        let signature = key.sign(&Self::signing_bytes(game_id, ply, &mv, played_at));
        AuthorizedMove {
            ply,
            mv,
            played_at,
            player: key.verifying_key(),
            signature,
        }
    }

    /// Check the signature against the embedded key. This says the move is
    /// authentic, *not* that its author was entitled to make it — that is
    /// [`MovesV1::verify`]'s job.
    pub fn verify_signature(&self, game_id: &GameId) -> Result<(), String> {
        if self.played_at <= 0 {
            return Err(format!(
                "move at ply {} has non-positive timestamp",
                self.ply
            ));
        }
        verify_sig(
            &self.player,
            &Self::signing_bytes(game_id, self.ply, &self.mv, self.played_at),
            &self.signature,
            &format!("move at ply {}", self.ply),
        )
    }

    /// Total order used to settle a double-signed ply. Deterministic and
    /// independent of arrival order.
    fn tiebreak(&self) -> [u8; 64] {
        self.signature.to_bytes()
    }
}

/// The move list, indexed by ply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovesV1 {
    /// `BTreeMap` rather than `HashMap`: the summary derives from these keys
    /// and must serialize in a deterministic order, or core's byte-level
    /// convergence check misfires.
    pub moves: BTreeMap<u32, AuthorizedMove>,
}

impl MovesV1 {
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    pub fn len(&self) -> usize {
        self.moves.len()
    }

    /// The contiguous run of moves from ply 0, as plain engine moves.
    pub fn move_list(&self) -> Vec<Move> {
        let mut out = Vec::with_capacity(self.moves.len());
        for ply in 0.. {
            match self.moves.get(&ply) {
                Some(am) => out.push(am.mv),
                None => break,
            }
        }
        out
    }

    /// Timestamps of the contiguous run, parallel to [`move_list`](Self::move_list).
    pub fn timestamps(&self) -> Vec<i64> {
        let mut out = Vec::with_capacity(self.moves.len());
        for ply in 0.. {
            match self.moves.get(&ply) {
                Some(am) => out.push(am.played_at),
                None => break,
            }
        }
        out
    }

    /// Merge one move in, settling a same-ply conflict deterministically.
    fn absorb(&mut self, incoming: AuthorizedMove) {
        match self.moves.get(&incoming.ply) {
            None => {
                self.moves.insert(incoming.ply, incoming);
            }
            Some(existing) => {
                if incoming.tiebreak() < existing.tiebreak() {
                    self.moves.insert(incoming.ply, incoming);
                }
            }
        }
    }

    /// Keep only the longest prefix that is authentic, correctly attributed,
    /// chronological and legal. Pure function of the current contents, so it is
    /// idempotent and identical on every peer.
    ///
    /// `keys` is `(white, black)`; `None` means no opponent has joined, in
    /// which case no move is admissible at all.
    pub(super) fn prune(&mut self, game_id: &GameId, keys: Option<(VerifyingKey, VerifyingKey)>) {
        let keys = match keys {
            Some(k) => k,
            None => {
                // No second player: a game with no opponent has no legal moves.
                self.moves.clear();
                return;
            }
        };

        let mut game = Game::new();
        let mut last_timestamp = i64::MIN;
        let mut keep = 0u32;

        for ply in 0.. {
            let candidate = match self.moves.get(&ply) {
                Some(c) => c,
                // A gap ends the prefix; later moves are orphans.
                None => break,
            };
            let expected = if ply % 2 == 0 { keys.0 } else { keys.1 };
            if candidate.player != expected
                || candidate.ply != ply
                || candidate.verify_signature(game_id).is_err()
                || candidate.played_at < last_timestamp
                || game.play(candidate.mv).is_err()
            {
                break;
            }
            last_timestamp = candidate.played_at;
            keep = ply + 1;
        }

        self.moves.retain(|ply, _| *ply < keep);
    }

    /// Full validation of the list against the two players. Unlike
    /// [`prune`](Self::prune) this rejects rather than repairs, so a state that
    /// arrives as a full PUT (which bypasses the merge path) cannot smuggle in
    /// moves that a merged state would have dropped.
    fn check(
        &self,
        game_id: &GameId,
        keys: Option<(VerifyingKey, VerifyingKey)>,
    ) -> Result<(), String> {
        if self.moves.is_empty() {
            return Ok(());
        }
        let (white, black) =
            keys.ok_or_else(|| "moves recorded before an opponent joined the game".to_string())?;

        // Contiguity: plies must be exactly 0..n with no gaps, otherwise the
        // list does not describe a game.
        let expected: BTreeSet<u32> = (0..self.moves.len() as u32).collect();
        let actual: BTreeSet<u32> = self.moves.keys().copied().collect();
        if expected != actual {
            return Err("move list has gaps or out-of-range plies".to_string());
        }

        let mut game = Game::new();
        let mut last_timestamp = i64::MIN;
        for (ply, am) in &self.moves {
            if am.ply != *ply {
                return Err(format!("move stored at ply {ply} claims ply {}", am.ply));
            }
            let expected_key = if ply % 2 == 0 { white } else { black };
            if am.player != expected_key {
                return Err(format!(
                    "move at ply {ply} was signed by a key that does not own the side to move"
                ));
            }
            am.verify_signature(game_id)?;
            if am.played_at < last_timestamp {
                return Err(format!("move at ply {ply} goes backwards in time"));
            }
            last_timestamp = am.played_at;
            game.play(am.mv)
                .map_err(|e| format!("illegal move at ply {ply}: {e}"))?;
        }
        Ok(())
    }
}

impl ComposableState for MovesV1 {
    type ParentState = ChessGameStateV1;
    type Summary = Vec<u32>;
    type Delta = Vec<AuthorizedMove>;
    type Parameters = ChessGameParametersV1;

    fn verify(&self, parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        self.check(&params.game_id, parent.player_keys())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        // Ordered by construction (`BTreeMap`), so the encoded summary bytes
        // are deterministic for a given set of plies.
        self.moves.keys().copied().collect()
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let theirs: BTreeSet<u32> = old_summary.iter().copied().collect();
        let missing: Vec<AuthorizedMove> = self
            .moves
            .iter()
            .filter(|(ply, _)| !theirs.contains(ply))
            .map(|(_, am)| am.clone())
            .collect();
        if missing.is_empty() {
            None
        } else {
            Some(missing)
        }
    }

    fn apply_delta(
        &mut self,
        _parent: &Self::ParentState,
        params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(incoming) = delta {
            for am in incoming {
                // Reject forgeries outright; attribution and legality are
                // settled by the prune pass once the whole state is merged,
                // because they need the opponent slot which may be arriving in
                // the same delta.
                am.verify_signature(&params.game_id)?;
                self.absorb(am.clone());
            }
        }
        Ok(())
    }
}
