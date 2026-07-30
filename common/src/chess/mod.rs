//! A complete, self-contained chess engine.
//!
//! No dependencies beyond `serde`, so the identical rule set compiles into the
//! contract WASM (which validates moves on untrusted peers), the delegate and
//! the browser UI. A move is only ever stored after this code accepted it.

pub mod board;
pub mod fen;
pub mod game;
pub mod types;

#[cfg(test)]
mod tests;

pub use board::{Board, MoveError, MoveOutcome};
pub use fen::{FenError, STARTING_FEN};
pub use game::Game;
pub use types::{CastlingRights, Color, GameStatus, Move, Piece, PieceKind, Square};
