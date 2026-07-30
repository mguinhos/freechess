//! Replay of a move list into a position, with the history-dependent rules
//! (threefold repetition) the board alone cannot judge.
//!
//! Both sides of the app run this same code: the contract replays a game's
//! signed move list to reject an illegal move before it can enter state, and
//! the UI replays it to drive the board, the move list and the replay scrubber.

use super::board::{Board, MoveError, MoveOutcome};
use super::types::{Color, GameStatus, Move};
use std::collections::BTreeMap;

/// A game as a starting position plus the moves played from it.
#[derive(Debug, Clone)]
pub struct Game {
    board: Board,
    moves: Vec<Move>,
    outcomes: Vec<MoveOutcome>,
    /// FEN after each ply, with index 0 being the position before any move.
    /// Kept so the replay scrubber can jump to any ply without re-replaying.
    fens: Vec<String>,
    position_counts: BTreeMap<String, u32>,
    status: GameStatus,
}

impl Default for Game {
    fn default() -> Self {
        Game::new()
    }
}

impl Game {
    /// A game from the standard starting position.
    pub fn new() -> Game {
        Game::from_board(Board::starting_position())
    }

    pub fn from_board(board: Board) -> Game {
        let mut position_counts = BTreeMap::new();
        position_counts.insert(board.position_key(), 1);
        let status = board.status(1);
        let fens = vec![board.to_fen()];
        Game {
            board,
            moves: Vec::new(),
            outcomes: Vec::new(),
            fens,
            position_counts,
            status,
        }
    }

    /// Replay a move list, rejecting the first illegal move. The `usize` in the
    /// error is the ply index that failed, which the contract reports back so a
    /// client can tell *which* move was rejected.
    pub fn from_moves(moves: &[Move]) -> Result<Game, (usize, MoveError)> {
        let mut game = Game::new();
        for (i, mv) in moves.iter().enumerate() {
            game.play(*mv).map_err(|e| (i, e))?;
        }
        Ok(game)
    }

    /// Play one move, updating status and repetition bookkeeping.
    pub fn play(&mut self, mv: Move) -> Result<&MoveOutcome, MoveError> {
        if self.status.is_over() {
            return Err(MoveError::GameOver);
        }
        let outcome = self.board.make_move(mv)?;

        // A capture or pawn move makes every earlier position unreachable, so
        // the repetition table can be cleared — this also keeps it bounded.
        if outcome.captured.is_some() || self.board.halfmove_clock == 0 {
            self.position_counts.clear();
        }
        let key = self.board.position_key();
        let count = self.position_counts.entry(key).or_insert(0);
        *count += 1;
        let repetitions = *count;

        self.status = self.board.status(repetitions);
        self.moves.push(mv);
        self.outcomes.push(outcome);
        self.fens.push(self.board.to_fen());
        Ok(self.outcomes.last().expect("just pushed"))
    }

    /// Is `mv` playable right now? Cheap enough for the UI to call per drag.
    pub fn is_legal(&self, mv: Move) -> bool {
        !self.status.is_over() && self.board.legal_moves().contains(&mv)
    }

    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn moves(&self) -> &[Move] {
        &self.moves
    }

    pub fn outcomes(&self) -> &[MoveOutcome] {
        &self.outcomes
    }

    pub fn status(&self) -> GameStatus {
        self.status
    }

    pub fn side_to_move(&self) -> Color {
        self.board.side_to_move
    }

    /// Number of plies played.
    pub fn ply(&self) -> usize {
        self.moves.len()
    }

    /// FEN after `ply` half-moves; `ply == 0` is the starting position. Returns
    /// the final position for any ply past the end.
    pub fn fen_at(&self, ply: usize) -> &str {
        let idx = ply.min(self.fens.len().saturating_sub(1));
        &self.fens[idx]
    }

    pub fn current_fen(&self) -> &str {
        self.fens.last().map(|s| s.as_str()).unwrap_or("")
    }

    /// SAN for every move played, in order.
    pub fn san_list(&self) -> Vec<String> {
        self.outcomes.iter().map(|o| o.san.clone()).collect()
    }

    /// The PGN result token for the current status. A game still in progress —
    /// or one whose outcome was decided off-board (resignation, timeout) — is
    /// `*`; callers with that knowledge pass their own token to [`to_pgn`].
    pub fn result_token(&self) -> &'static str {
        match self.status {
            GameStatus::Checkmate {
                winner: Color::White,
            } => "1-0",
            GameStatus::Checkmate {
                winner: Color::Black,
            } => "0-1",
            GameStatus::Stalemate
            | GameStatus::InsufficientMaterial
            | GameStatus::FiftyMoveRule
            | GameStatus::ThreefoldRepetition => "1/2-1/2",
            GameStatus::InProgress => "*",
        }
    }

    /// Export as PGN. `tags` are emitted in the order given, before the
    /// mandatory `Result` tag.
    pub fn to_pgn(&self, tags: &[(&str, String)], result: &str) -> String {
        let mut pgn = String::new();
        for (name, value) in tags {
            // PGN tag values are quoted; escape the two characters that would
            // otherwise break the token.
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            pgn.push_str(&format!("[{name} \"{escaped}\"]\n"));
        }
        pgn.push_str(&format!("[Result \"{result}\"]\n\n"));

        let mut line = String::new();
        for (i, san) in self.san_list().iter().enumerate() {
            if i % 2 == 0 {
                line.push_str(&format!("{}. ", i / 2 + 1));
            }
            line.push_str(san);
            line.push(' ');
            // Keep lines under the 80-column PGN convention.
            if line.len() > 70 {
                pgn.push_str(line.trim_end());
                pgn.push('\n');
                line.clear();
            }
        }
        line.push_str(result);
        pgn.push_str(line.trim_end());
        pgn.push('\n');
        pgn
    }
}
