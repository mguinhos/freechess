//! The game contract's state: one instance per chess game.
//!
//! # Authorization model
//!
//! Exactly two keys may ever move in a game, and each may only move on its own
//! turn:
//!
//! * The **creator** is fixed in the contract *parameters*, so it is part of the
//!   contract key itself and cannot be changed by anyone.
//! * The **opponent** is whoever wins the join race ([`OpponentSlotV1`]). Once
//!   the state converges, that slot is immutable — a third party cannot
//!   displace it, because the merge picks the winner by a deterministic rule
//!   over the *set* of joins, not by arrival order.
//! * Every move carries a signature ([`moves::AuthorizedMove`]) that must come
//!   from the key owning the side to move at that ply. A spectator, or a peer
//!   relaying the state, has no key that satisfies this, so no third party can
//!   ever inject a move. See [`moves::MovesV1`].
//!
//! # Anti-spam
//!
//! Creating a game costs proof-of-work: the `game_id` in the parameters must be
//! a hash of the creator's key and a nonce with a required number of leading
//! zero bits ([`setup::GameSetup`]). Because `game_id` is a contract parameter,
//! the work is bound to the contract key itself and is re-verifiable by every
//! peer with no shared state. The lobby applies a second, per-creator quota on
//! top — see [`crate::lobby`].
//!
//! # Everything here is public
//!
//! All of this state is replicated to every peer that routes near the
//! contract's ring location. Spectators need no permission: they subscribe to
//! the same contract the players use.

pub mod clocks;
pub mod conclusion;
pub mod moves;
pub mod opponent;
pub mod setup;

#[cfg(test)]
mod tests;

use crate::chess::{Color, Game, GameStatus};
use crate::identity::{GameId, PlayerId};
use clocks::ClocksV1;
use conclusion::ConclusionV1;
use ed25519_dalek::VerifyingKey;
use freenet_scaffold_macro::composable;
use moves::MovesV1;
use opponent::OpponentSlotV1;
use serde::{Deserialize, Serialize};
use setup::GameSetupV1;

/// Domain separator prefix for every signature in this crate. Including the
/// purpose in the signed bytes stops a signature made for one message type from
/// being replayed as another.
pub const SIG_DOMAIN: &[u8] = b"freechess:v1:";

/// Parameters fixed at contract creation; together with the WASM hash they form
/// the contract key, so neither can be altered after publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChessGameParametersV1 {
    /// The player who created the game. Immutable by construction.
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub creator: VerifyingKey,
    /// Proof-of-work identifier binding the game to its creator. See
    /// [`setup::GameSetup::derive_game_id`].
    pub game_id: GameId,
}

impl ChessGameParametersV1 {
    pub fn creator_id(&self) -> PlayerId {
        PlayerId::from(&self.creator)
    }
}

/// A chess game as replicated state.
///
/// Field order matters to the `#[composable]` macro: deltas are applied in
/// declaration order, and each field's `verify` may read the fields before it.
/// `setup` must come first (it establishes who the creator is playing as),
/// then `opponent` (which completes the pair of keys), then `moves` (which
/// needs both), then `clocks` (which needs both keys to know whose attestations
/// count), then `conclusion` (which needs the move list and the attestations to
/// judge a timeout claim).
#[composable(post_apply_delta = "prune")]
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
pub struct ChessGameStateV1 {
    /// Signed by the creator: colors, time control, proof-of-work nonce.
    pub setup: GameSetupV1,

    /// The second player. Empty until someone joins; immutable afterwards.
    pub opponent: OpponentSlotV1,

    /// The signed move list, indexed by ply.
    pub moves: MovesV1,

    /// Each player's signed statement of the time they see. The only evidence
    /// in state that time has actually passed — see [`clocks`].
    pub clocks: ClocksV1,

    /// Resignation, agreed draw, or a timeout claim. Empty while the game is
    /// decided purely on the board.
    pub conclusion: ConclusionV1,
}

/// How a game finished, combining the on-board result with the off-board ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameResult {
    InProgress,
    /// Nobody joined yet — the game is open for a challenger.
    AwaitingOpponent,
    WhiteWins(WinReason),
    BlackWins(WinReason),
    Draw(DrawReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WinReason {
    Checkmate,
    Resignation,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrawReason {
    Stalemate,
    Agreement,
    InsufficientMaterial,
    FiftyMoveRule,
    ThreefoldRepetition,
}

impl GameResult {
    pub fn is_over(self) -> bool {
        !matches!(self, GameResult::InProgress | GameResult::AwaitingOpponent)
    }

    /// PGN result token.
    pub fn pgn_token(self) -> &'static str {
        match self {
            GameResult::WhiteWins(_) => "1-0",
            GameResult::BlackWins(_) => "0-1",
            GameResult::Draw(_) => "1/2-1/2",
            GameResult::InProgress | GameResult::AwaitingOpponent => "*",
        }
    }

    /// The winner's score from White's point of view, used for Elo.
    pub fn score_for_white(self) -> Option<crate::elo::Score> {
        match self {
            GameResult::WhiteWins(_) => Some(crate::elo::Score::Win),
            GameResult::BlackWins(_) => Some(crate::elo::Score::Loss),
            GameResult::Draw(_) => Some(crate::elo::Score::Draw),
            GameResult::InProgress | GameResult::AwaitingOpponent => None,
        }
    }
}

impl ChessGameStateV1 {
    /// Drop anything the merged state cannot justify.
    ///
    /// Runs after every delta application. It is a pure, idempotent function of
    /// the merged contents — never of arrival order — which is what the
    /// whitepaper requires of any eviction rule for the state to stay a
    /// join-semilattice.
    ///
    /// The one thing it prunes is the move list: moves whose signer does not
    /// own the side to move, moves that are illegal, moves after a gap, and any
    /// move at all when no opponent has joined. `verify` rejects those same
    /// states outright, which covers the full-state PUT path that skips this
    /// hook.
    pub fn prune(&mut self, params: &ChessGameParametersV1) -> Result<(), String> {
        let keys = self.player_keys();
        self.moves.prune(&params.game_id, keys);
        self.clocks.prune(keys);
        Ok(())
    }

    /// Milliseconds `color` has spent on their own moves so far.
    ///
    /// Each move consumes the interval since the opponent's previous move (or
    /// since the game started, for White's first). Derived entirely from signed
    /// timestamps in state, so every peer computes the same clock.
    pub fn time_used_by(&self, color: Color) -> i64 {
        self.time_used_through(color, usize::MAX)
    }

    /// As [`time_used_by`](Self::time_used_by), counting only the first `plies`
    /// half-moves.
    ///
    /// Timeout claims are judged at the ply they name rather than at the head of
    /// the list, so that a move arriving afterwards cannot retroactively rewrite
    /// the clock the claim was measured against.
    fn time_used_through(&self, color: Color, plies: usize) -> i64 {
        let stamps = self.moves.timestamps();
        let stamps = &stamps[..plies.min(stamps.len())];
        let start = match self.game_start_time() {
            Some(t) => t,
            None => return 0,
        };
        let mut used = 0i64;
        for (ply, stamp) in stamps.iter().enumerate() {
            let mover = if ply % 2 == 0 {
                Color::White
            } else {
                Color::Black
            };
            if mover != color {
                continue;
            }
            let previous = if ply == 0 { start } else { stamps[ply - 1] };
            used += (stamp - previous).max(0);
        }
        used
    }

    /// How many moves `color` has played, which is how much increment they have
    /// earned.
    pub fn moves_made_by(&self, color: Color) -> u32 {
        self.moves_made_through(color, usize::MAX)
    }

    /// As [`moves_made_by`](Self::moves_made_by), counting only the first
    /// `plies` half-moves.
    fn moves_made_through(&self, color: Color, plies: usize) -> u32 {
        let total = self.moves.move_list().len().min(plies) as u32;
        match color {
            Color::White => total.div_ceil(2),
            Color::Black => total / 2,
        }
    }

    /// Milliseconds `color` starts with, plus the increment earned over the
    /// first `plies` half-moves.
    fn budget_through(&self, color: Color, plies: usize) -> Option<i64> {
        let tc = self.setup.get()?.setup.time_control;
        Some(
            tc.initial_secs as i64 * 1000
                + tc.increment_secs as i64 * 1000 * self.moves_made_through(color, plies) as i64,
        )
    }

    /// The earliest instant at which `color` losing on time at `at_ply` follows
    /// from state alone, or `None` if it does not follow at all.
    ///
    /// This is the whole of the timeout rule. A contract has no clock, so the
    /// deadline cannot be taken from the claimant's word for what time it is —
    /// it is derived here from the loser's *own* signed attestations, which are
    /// the only evidence about elapsed time that the beneficiary of a timeout
    /// cannot have written. See [`clocks`] for why that is the only sound
    /// source and what it does and does not guarantee.
    ///
    /// Like [`pending_time_for`](Self::pending_time_for), this deliberately
    /// reads only the board, the move timestamps and the attestations — never
    /// [`result`](Self::result). A timeout claim is judged against the state
    /// that already contains it, so consulting the conclusion here would make
    /// every claim invalidate itself.
    pub fn timeout_provable_at(&self, color: Color, at_ply: u32) -> Option<i64> {
        let (white, black) = self.player_keys()?;
        let plies = at_ply as usize;
        let stamps = self.moves.timestamps();
        // A claim pointing past the moves in state has nothing to be judged
        // against yet.
        if plies > stamps.len() {
            return None;
        }
        // Only the side to move at that ply can have been running their clock
        // down.
        let to_move = if plies % 2 == 0 {
            Color::White
        } else {
            Color::Black
        };
        if to_move != color {
            return None;
        }
        // A position the board already decided was not decided on time.
        let played = self.moves.move_list();
        let position = Game::from_moves(&played[..plies]).ok()?;
        if position.status().is_over() {
            return None;
        }

        let since = if plies == 0 {
            self.game_start_time()?
        } else {
            stamps[plies - 1]
        };
        let remaining =
            (self.budget_through(color, plies)? - self.time_used_through(color, plies)).max(0);
        let flag_fall = since.saturating_add(remaining);

        let key = match color {
            Color::White => white,
            Color::Black => black,
        };
        // Floor at the moment their turn began, since they were demonstrably
        // there when it did; ceiling at their own flag fall, so attesting to a
        // far-future time buys nothing beyond the clock they already had.
        let attested = self
            .clocks
            .attested_at(&key)
            .unwrap_or(i64::MIN)
            .max(since)
            .min(flag_fall);

        Some(if attested >= flag_fall {
            // Their own signatures reach the moment their clock hit zero: they
            // were present, and they ran out of time.
            flag_fall
        } else {
            // Their signatures stopped while they still had time on the clock.
            attested.saturating_add(clocks::ABSENCE_FORFEIT_MS)
        })
    }

    /// Time `color` has burned on the move they are currently thinking about,
    /// measured up to `now`. Zero when it is not their turn.
    ///
    /// Deliberately judges "is the game still running" from the *board* alone,
    /// not from [`result`](Self::result). A timeout claim is verified against
    /// the state that already contains it, so consulting the conclusion here
    /// would make every timeout claim invalidate itself.
    pub fn pending_time_for(&self, color: Color, now: i64) -> i64 {
        if self.replay().status().is_over() || self.opponent.get().is_none() {
            return 0;
        }
        let stamps = self.moves.timestamps();
        let to_move = if stamps.len() % 2 == 0 {
            Color::White
        } else {
            Color::Black
        };
        if to_move != color {
            return 0;
        }
        let since = match stamps.last() {
            Some(last) => *last,
            None => match self.game_start_time() {
                Some(t) => t,
                None => return 0,
            },
        };
        (now - since).max(0)
    }

    /// Milliseconds left on `color`'s clock at `now`, floored at zero.
    pub fn time_remaining(&self, color: Color, now: i64) -> i64 {
        let budget = match self.budget_through(color, usize::MAX) {
            Some(b) => b,
            None => return 0,
        };
        let spent = self.time_used_by(color) + self.pending_time_for(color, now);
        (budget - spent).max(0)
    }

    /// When the clocks started: the moment the opponent joined, since a game
    /// with one player is not running.
    fn game_start_time(&self) -> Option<i64> {
        let joined = self.opponent.get()?.joined_at;
        let created = self.setup.get()?.setup.created_at;
        // Guard against a challenger back-dating their join to steal clock time
        // from the creator.
        Some(joined.max(created))
    }

    /// The two players' keys, once both are known.
    ///
    /// `None` until someone has joined — which is exactly the condition under
    /// which no move may be played.
    pub fn player_keys(&self) -> Option<(VerifyingKey, VerifyingKey)> {
        let setup = self.setup.get()?;
        let opponent = self.opponent.get()?;
        let creator = setup.creator_key();
        Some(match setup.setup.creator_plays {
            Color::White => (creator, opponent.player),
            Color::Black => (opponent.player, creator),
        })
    }

    /// The key that must sign the move at `ply`, or `None` if the game has no
    /// opponent yet. White plays even plies.
    pub fn key_for_ply(&self, ply: u32) -> Option<VerifyingKey> {
        let (white, black) = self.player_keys()?;
        Some(if ply % 2 == 0 { white } else { black })
    }

    /// Which color a given key plays, if it is one of the two players.
    pub fn color_of(&self, key: &VerifyingKey) -> Option<Color> {
        let (white, black) = self.player_keys()?;
        if key == &white {
            Some(Color::White)
        } else if key == &black {
            Some(Color::Black)
        } else {
            None
        }
    }

    /// Replay the signed move list into a position.
    ///
    /// Returns the engine's `Game`, which carries the board, SAN list and every
    /// intermediate FEN — this single call drives the board, the move list, the
    /// spectator view and the replay scrubber.
    pub fn replay(&self) -> Game {
        // `MovesV1` only ever holds a contiguous, legal prefix (its merge
        // truncates at the first move that fails), so replay cannot fail here.
        Game::from_moves(&self.moves.move_list()).unwrap_or_else(|_| Game::new())
    }

    /// The game's outcome, combining board state with any signed conclusion.
    pub fn result(&self) -> GameResult {
        if self.setup.get().is_none() {
            return GameResult::InProgress;
        }
        if self.opponent.get().is_none() {
            return GameResult::AwaitingOpponent;
        }

        // A signed conclusion (resignation, agreed draw, timeout) outranks the
        // board, since it is how a game ends off-board.
        if let Some(result) = self.conclusion.result_with(self) {
            return result;
        }

        match self.replay().status() {
            GameStatus::InProgress => GameResult::InProgress,
            GameStatus::Checkmate {
                winner: Color::White,
            } => GameResult::WhiteWins(WinReason::Checkmate),
            GameStatus::Checkmate {
                winner: Color::Black,
            } => GameResult::BlackWins(WinReason::Checkmate),
            GameStatus::Stalemate => GameResult::Draw(DrawReason::Stalemate),
            GameStatus::InsufficientMaterial => GameResult::Draw(DrawReason::InsufficientMaterial),
            GameStatus::FiftyMoveRule => GameResult::Draw(DrawReason::FiftyMoveRule),
            GameStatus::ThreefoldRepetition => GameResult::Draw(DrawReason::ThreefoldRepetition),
        }
    }

    /// Is this game accepting a challenger?
    pub fn is_open(&self) -> bool {
        self.setup.get().is_some() && self.opponent.get().is_none()
    }

    /// FEN of the current position — what the lobby's mini-boards render.
    pub fn current_fen(&self) -> String {
        self.replay().current_fen().to_string()
    }
}
