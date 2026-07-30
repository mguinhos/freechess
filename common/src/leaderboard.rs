//! The Elo ranking, carried by the lobby contract.
//!
//! # Verifying a rating in O(1)
//!
//! A rating is only meaningful if a peer can check it without trusting the
//! player who published it — but replaying someone's whole history would mean
//! fetching every game they ever played.
//!
//! The [`GameCertificate`] closes that gap. It is co-signed by *both* players
//! and records the rating each of them held going into the game. So a rank
//! entry ships its owner's most recent certificate, and any peer checks:
//!
//! ```text
//! claimed_rating == elo::apply_result(rating_before, games_played - 1,
//!                                     opponent_rating, score)
//! ```
//!
//! where `rating_before` and `opponent_rating` both come from bytes the
//! *opponent* signed. A player cannot inflate their own rating without an
//! opponent willing to co-sign the inflated starting point, and each such step
//! costs a real game with proof-of-work behind it.
//!
//! This does not stop two colluding keys from farming each other — see the
//! discussion in [`crate::certificate`]. It does mean a rating is never simply
//! taken on faith.

use crate::certificate::GameCertificate;
use crate::elo::{self, STARTING_ELO};
use crate::game::setup::MAX_NICKNAME_LEN;
use crate::game::SIG_DOMAIN;
use crate::identity::{signature_digest, verify_sig, PlayerId};
use crate::lobby::{LobbyParametersV1, LobbyStateV1};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Players the ranking holds. Beyond this the lowest-rated are evicted, which
/// bounds state while keeping the table everyone actually looks at.
pub const LEADERBOARD_SIZE: usize = 200;

/// A player's place in the ranking, with the proof that backs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankEntry {
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub player: VerifyingKey,
    pub nickname: String,
    pub rating: i32,
    pub games_played: u32,
    /// The player's most recent certified game. Both the rating this entry
    /// claims and the identity of the player are checked against it.
    pub last_game: GameCertificate,
    pub updated_at: i64,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl RankEntry {
    fn signing_bytes(
        player: &VerifyingKey,
        nickname: &str,
        rating: i32,
        games_played: u32,
        last_game_id: &crate::identity::GameId,
        updated_at: i64,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(SIG_DOMAIN);
        buf.extend_from_slice(b"rank:");
        buf.extend_from_slice(player.as_bytes());
        buf.extend_from_slice(&(nickname.len() as u32).to_le_bytes());
        buf.extend_from_slice(nickname.as_bytes());
        buf.extend_from_slice(&rating.to_le_bytes());
        buf.extend_from_slice(&games_played.to_le_bytes());
        buf.extend_from_slice(&last_game_id.0);
        buf.extend_from_slice(&updated_at.to_le_bytes());
        buf
    }

    /// Publish a place in the ranking.
    pub fn new(
        key: &SigningKey,
        nickname: String,
        rating: i32,
        games_played: u32,
        last_game: GameCertificate,
        updated_at: i64,
    ) -> RankEntry {
        let player = key.verifying_key();
        let signature = key.sign(&Self::signing_bytes(
            &player,
            &nickname,
            rating,
            games_played,
            &last_game.game_id,
            updated_at,
        ));
        RankEntry {
            player,
            nickname,
            rating,
            games_played,
            last_game,
            updated_at,
            signature,
        }
    }

    pub fn player_id(&self) -> PlayerId {
        PlayerId::from(&self.player)
    }

    /// Check the entry end to end: the player's signature, the certificate it
    /// leans on, and the arithmetic connecting the two.
    pub fn verify(&self) -> Result<(), String> {
        if self.nickname.len() > MAX_NICKNAME_LEN {
            return Err(format!("nickname longer than {MAX_NICKNAME_LEN} bytes"));
        }
        if self.nickname.trim().is_empty() {
            return Err("nickname must not be blank".to_string());
        }
        if self.games_played == 0 {
            return Err("a ranked player must have played at least one game".to_string());
        }
        if self.rating < elo::MIN_ELO {
            return Err("rating below the floor".to_string());
        }

        verify_sig(
            &self.player,
            &Self::signing_bytes(
                &self.player,
                &self.nickname,
                self.rating,
                self.games_played,
                &self.last_game.game_id,
                self.updated_at,
            ),
            &self.signature,
            "rank entry",
        )?;

        // The certificate must be genuine and must actually involve this
        // player — otherwise anyone could borrow a stranger's game as proof.
        self.last_game.verify()?;
        let me = self.player_id();
        if !self.last_game.involves(me) {
            return Err("rank entry cites a game its player did not take part in".to_string());
        }

        // The claimed rating must be exactly what the Elo formula yields from
        // the co-signed inputs. This is the check that makes the number
        // trustworthy rather than self-declared.
        let rating_before = self
            .last_game
            .color_of(me)
            .map(|c| match c {
                crate::chess::Color::White => self.last_game.white_rating_before,
                crate::chess::Color::Black => self.last_game.black_rating_before,
            })
            .ok_or_else(|| "player is not in the cited game".to_string())?;
        let opponent_rating = self
            .last_game
            .opponent_rating_for(me)
            .ok_or_else(|| "cited game has no opponent rating".to_string())?;
        let score = self
            .last_game
            .score_for(me)
            .ok_or_else(|| "cited game has no decided result".to_string())?;

        let expected = elo::apply_result(
            rating_before,
            self.games_played.saturating_sub(1),
            opponent_rating,
            score,
        );
        if expected != self.rating {
            return Err(format!(
                "rating {} does not follow from the cited game (expected {expected})",
                self.rating
            ));
        }
        Ok(())
    }

    /// Newest wins, with a deterministic tiebreak.
    fn order_key(&self) -> (u32, i64, [u8; 64]) {
        (
            self.games_played,
            self.updated_at,
            self.signature.to_bytes(),
        )
    }

    /// Ranking order: rating descending, player id ascending for stability.
    fn rank_key(&self) -> (std::cmp::Reverse<i32>, [u8; 16]) {
        (std::cmp::Reverse(self.rating), self.player_id().0)
    }
}

/// The ranking table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderboardV1 {
    pub entries: BTreeMap<PlayerId, RankEntry>,
}

impl LeaderboardV1 {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, player: PlayerId) -> Option<&RankEntry> {
        self.entries.get(&player)
    }

    /// A player's rating, or the starting rating if they are unranked.
    pub fn rating_of(&self, player: PlayerId) -> i32 {
        self.entries
            .get(&player)
            .map(|e| e.rating)
            .unwrap_or(STARTING_ELO)
    }

    /// The table in rank order, best first.
    pub fn ranked(&self) -> Vec<&RankEntry> {
        let mut out: Vec<&RankEntry> = self.entries.values().collect();
        out.sort_by_key(|e| e.rank_key());
        out
    }

    /// A player's 1-based position in the table, if ranked.
    pub fn position_of(&self, player: PlayerId) -> Option<usize> {
        self.ranked()
            .iter()
            .position(|e| e.player_id() == player)
            .map(|i| i + 1)
    }

    fn absorb(&mut self, incoming: RankEntry) {
        let id = incoming.player_id();
        match self.entries.get(&id) {
            None => {
                self.entries.insert(id, incoming);
            }
            Some(current) => {
                // More games played wins: a rating only moves forward as games
                // are played, so this is monotone and therefore convergent.
                if incoming.order_key() > current.order_key() {
                    self.entries.insert(id, incoming);
                }
            }
        }
    }

    /// Keep the top [`LEADERBOARD_SIZE`] by a total order. Pure, idempotent,
    /// order-independent.
    fn enforce_cap(&mut self) {
        if self.entries.len() <= LEADERBOARD_SIZE {
            return;
        }
        let mut ids: Vec<PlayerId> = self.entries.keys().copied().collect();
        ids.sort_by_key(|id| self.entries[id].rank_key());
        for id in ids.into_iter().skip(LEADERBOARD_SIZE) {
            self.entries.remove(&id);
        }
    }

    pub(crate) fn prune(&mut self) {
        self.enforce_cap();
    }
}

impl ComposableState for LeaderboardV1 {
    type ParentState = LobbyStateV1;
    /// Per player, a fingerprint of the entry we hold. Games played alone was
    /// coarser than the order `absorb` imposes, so a fresher entry with the same
    /// count never shipped and two peers kept different ratings for ever.
    type Summary = Vec<(PlayerId, u64)>;
    type Delta = Vec<RankEntry>;
    type Parameters = LobbyParametersV1;

    fn verify(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
    ) -> Result<(), String> {
        if self.entries.len() > LEADERBOARD_SIZE {
            return Err(format!(
                "leaderboard holds {} entries, over the {LEADERBOARD_SIZE} cap",
                self.entries.len()
            ));
        }
        for (id, entry) in &self.entries {
            if &entry.player_id() != id {
                return Err("rank entry stored under the wrong player id".to_string());
            }
            entry.verify()?;
        }
        Ok(())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        self.entries
            .iter()
            .map(|(id, e)| (*id, signature_digest(&e.signature)))
            .collect()
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let theirs: BTreeMap<PlayerId, u64> = old_summary.iter().copied().collect();
        let changed: Vec<RankEntry> = self
            .entries
            .iter()
            .filter(|(id, e)| theirs.get(id) != Some(&signature_digest(&e.signature)))
            .map(|(_, e)| e.clone())
            .collect();
        if changed.is_empty() {
            None
        } else {
            Some(changed)
        }
    }

    fn apply_delta(
        &mut self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(entries) = delta {
            for entry in entries {
                entry.verify()?;
                self.absorb(entry.clone());
            }
        }
        Ok(())
    }
}
