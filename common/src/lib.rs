//! Shared types for FreeChess: the chess engine plus the state definitions
//! for the game, lobby and player contracts.
//!
//! Compiled into the contract WASMs, the delegate and the browser UI, so the
//! same rules and the same signature checks run everywhere.

pub mod account;
pub mod admin;
pub mod archive;
pub mod certificate;
pub mod chess;
pub mod delegate_api;
pub mod elo;
pub mod game;
pub mod identity;
pub mod leaderboard;
pub mod lobby;
pub mod player;
pub mod presence;
pub mod web_container;

#[cfg(test)]
mod testutil;

pub use archive::{ArchiveParametersV1, ArchiveStateV1, ShardId};
pub use certificate::{CertificateDraft, GameCertificate};
pub use chess::{Board, Color, Game, GameStatus, Move, Piece, PieceKind, Square};
pub use game::{ChessGameParametersV1, ChessGameStateV1, GameResult};
pub use identity::{GameId, PlayerId};
pub use leaderboard::{LeaderboardV1, RankEntry};
pub use lobby::{LobbyEntry, LobbyParametersV1, LobbyStateV1};
pub use player::{PlayerParametersV1, PlayerStateV1};
