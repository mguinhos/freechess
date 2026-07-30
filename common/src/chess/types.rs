//! Core chess value types: colors, pieces, squares, moves, castling rights.
//!
//! Everything here is dependency-free (beyond serde) so the same types compile
//! into the contract WASM, the delegate and the browser UI.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Which side a piece belongs to / whose turn it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Color {
    White,
    Black,
}

impl Color {
    pub fn opposite(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }

    /// Rank (0-indexed) a pawn of this color starts on.
    pub fn pawn_start_rank(self) -> u8 {
        match self {
            Color::White => 1,
            Color::Black => 6,
        }
    }

    /// Rank a pawn of this color promotes on.
    pub fn promotion_rank(self) -> u8 {
        match self {
            Color::White => 7,
            Color::Black => 0,
        }
    }

    /// Direction a pawn of this color advances, in ranks.
    pub fn pawn_direction(self) -> i8 {
        match self {
            Color::White => 1,
            Color::Black => -1,
        }
    }

    /// Back rank, where the king and rooks start.
    pub fn back_rank(self) -> u8 {
        match self {
            Color::White => 0,
            Color::Black => 7,
        }
    }
}

/// The kind of a piece, independent of color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PieceKind {
    Pawn,
    Knight,
    Bishop,
    Rook,
    Queen,
    King,
}

impl PieceKind {
    /// Lowercase FEN/SAN letter. Pawns have no SAN letter, hence the `'p'` is
    /// only meaningful in FEN context.
    pub fn letter(self) -> char {
        match self {
            PieceKind::Pawn => 'p',
            PieceKind::Knight => 'n',
            PieceKind::Bishop => 'b',
            PieceKind::Rook => 'r',
            PieceKind::Queen => 'q',
            PieceKind::King => 'k',
        }
    }

    pub fn from_letter(c: char) -> Option<PieceKind> {
        match c.to_ascii_lowercase() {
            'p' => Some(PieceKind::Pawn),
            'n' => Some(PieceKind::Knight),
            'b' => Some(PieceKind::Bishop),
            'r' => Some(PieceKind::Rook),
            'q' => Some(PieceKind::Queen),
            'k' => Some(PieceKind::King),
            _ => None,
        }
    }

    /// Conventional centipawn-ish material value, used only for the captured
    /// material display in the UI.
    pub fn material_value(self) -> u32 {
        match self {
            PieceKind::Pawn => 1,
            PieceKind::Knight | PieceKind::Bishop => 3,
            PieceKind::Rook => 5,
            PieceKind::Queen => 9,
            PieceKind::King => 0,
        }
    }
}

/// A colored piece sitting on a square.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Piece {
    pub color: Color,
    pub kind: PieceKind,
}

impl Piece {
    pub fn new(color: Color, kind: PieceKind) -> Piece {
        Piece { color, kind }
    }

    /// FEN character: uppercase for white, lowercase for black.
    pub fn fen_char(self) -> char {
        let c = self.kind.letter();
        match self.color {
            Color::White => c.to_ascii_uppercase(),
            Color::Black => c,
        }
    }

    pub fn from_fen_char(c: char) -> Option<Piece> {
        let kind = PieceKind::from_letter(c)?;
        let color = if c.is_ascii_uppercase() {
            Color::White
        } else {
            Color::Black
        };
        Some(Piece { color, kind })
    }
}

/// A board square, indexed 0..64 with a1 = 0, b1 = 1, ... h8 = 63.
///
/// Stored as a plain `u8` so a move fits in three bytes on the wire.
///
/// `Deserialize` is hand-written to enforce the range, because a `Square` is
/// used to index a fixed 64-element array. Doing it here rather than at each
/// indexing site covers every path a square can arrive by — a move in a game
/// delta, a move list inside a certificate — with one check, and makes an
/// out-of-range value unrepresentable in anything decoded from the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Square(pub u8);

/// Number of squares on a board; also the exclusive upper bound on a [`Square`].
pub const BOARD_SQUARES: u8 = 64;

impl<'de> Deserialize<'de> for Square {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Square, D::Error> {
        let index = u8::deserialize(d)?;
        if index >= BOARD_SQUARES {
            return Err(serde::de::Error::custom(format!(
                "square {index} is off the board (must be 0..{BOARD_SQUARES})"
            )));
        }
        Ok(Square(index))
    }
}

impl Square {
    /// Build from file (0 = a) and rank (0 = rank 1). Returns `None` if either
    /// coordinate is off the board.
    pub fn from_coords(file: i8, rank: i8) -> Option<Square> {
        if (0..8).contains(&file) && (0..8).contains(&rank) {
            Some(Square((rank * 8 + file) as u8))
        } else {
            None
        }
    }

    pub fn file(self) -> u8 {
        self.0 % 8
    }

    pub fn rank(self) -> u8 {
        self.0 / 8
    }

    /// Parse algebraic coordinates such as `e4`.
    pub fn from_algebraic(s: &str) -> Option<Square> {
        let bytes = s.as_bytes();
        if bytes.len() != 2 {
            return None;
        }
        let file = (bytes[0] as char).to_ascii_lowercase() as i8 - b'a' as i8;
        let rank = (bytes[1] as char) as i8 - b'1' as i8;
        Square::from_coords(file, rank)
    }

    /// The square is light-colored (used for board painting).
    pub fn is_light(self) -> bool {
        (self.file() + self.rank()) % 2 == 1
    }
}

impl fmt::Display for Square {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}",
            (b'a' + self.file()) as char,
            (b'1' + self.rank()) as char
        )
    }
}

/// A move in the compact form that gets signed and stored on the network.
///
/// Deliberately minimal — no captured piece, no check flag, no SAN. Everything
/// else is derived by replaying the move list against a [`Board`](super::Board),
/// which means a peer cannot lie about the consequences of a move, only about
/// the move itself (and that is caught by legality checking).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Move {
    pub from: Square,
    pub to: Square,
    /// Set only for pawn moves reaching the last rank.
    pub promotion: Option<PieceKind>,
}

impl Move {
    pub fn new(from: Square, to: Square) -> Move {
        Move {
            from,
            to,
            promotion: None,
        }
    }

    pub fn promoting(from: Square, to: Square, kind: PieceKind) -> Move {
        Move {
            from,
            to,
            promotion: Some(kind),
        }
    }

    /// Long algebraic / UCI form, e.g. `e2e4` or `e7e8q`.
    pub fn to_uci(self) -> String {
        match self.promotion {
            Some(k) => format!("{}{}{}", self.from, self.to, k.letter()),
            None => format!("{}{}", self.from, self.to),
        }
    }

    pub fn from_uci(s: &str) -> Option<Move> {
        if s.len() < 4 || s.len() > 5 {
            return None;
        }
        let from = Square::from_algebraic(&s[0..2])?;
        let to = Square::from_algebraic(&s[2..4])?;
        let promotion = if s.len() == 5 {
            let kind = PieceKind::from_letter(s.as_bytes()[4] as char)?;
            // Only real promotion pieces are legal targets.
            if matches!(
                kind,
                PieceKind::Knight | PieceKind::Bishop | PieceKind::Rook | PieceKind::Queen
            ) {
                Some(kind)
            } else {
                return None;
            }
        } else {
            None
        };
        Some(Move {
            from,
            to,
            promotion,
        })
    }

    /// The four bytes signed as part of a `SignedMove`. Fixed-width and
    /// version-prefixed by the caller.
    pub fn signing_bytes(self) -> [u8; 3] {
        let promo = match self.promotion {
            None => 0u8,
            Some(PieceKind::Knight) => 1,
            Some(PieceKind::Bishop) => 2,
            Some(PieceKind::Rook) => 3,
            Some(PieceKind::Queen) => 4,
            // Pawn/King are not legal promotion targets; encoding them as 0
            // would alias with "no promotion", so use distinct sentinels.
            Some(PieceKind::Pawn) => 5,
            Some(PieceKind::King) => 6,
        };
        [self.from.0, self.to.0, promo]
    }
}

impl fmt::Display for Move {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_uci())
    }
}

/// Which castles are still available to each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CastlingRights {
    pub white_kingside: bool,
    pub white_queenside: bool,
    pub black_kingside: bool,
    pub black_queenside: bool,
}

impl Default for CastlingRights {
    fn default() -> Self {
        CastlingRights {
            white_kingside: true,
            white_queenside: true,
            black_kingside: true,
            black_queenside: true,
        }
    }
}

impl CastlingRights {
    pub fn none() -> Self {
        CastlingRights {
            white_kingside: false,
            white_queenside: false,
            black_kingside: false,
            black_queenside: false,
        }
    }

    pub fn kingside(&self, color: Color) -> bool {
        match color {
            Color::White => self.white_kingside,
            Color::Black => self.black_kingside,
        }
    }

    pub fn queenside(&self, color: Color) -> bool {
        match color {
            Color::White => self.white_queenside,
            Color::Black => self.black_queenside,
        }
    }

    pub fn clear_for(&mut self, color: Color) {
        match color {
            Color::White => {
                self.white_kingside = false;
                self.white_queenside = false;
            }
            Color::Black => {
                self.black_kingside = false;
                self.black_queenside = false;
            }
        }
    }

    /// FEN castling field, `-` when nobody can castle.
    pub fn to_fen(self) -> String {
        let mut s = String::new();
        if self.white_kingside {
            s.push('K');
        }
        if self.white_queenside {
            s.push('Q');
        }
        if self.black_kingside {
            s.push('k');
        }
        if self.black_queenside {
            s.push('q');
        }
        if s.is_empty() {
            s.push('-');
        }
        s
    }
}

/// Why a game ended, or that it is still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameStatus {
    InProgress,
    /// The side to move has no legal moves and is in check.
    Checkmate {
        winner: Color,
    },
    Stalemate,
    /// Neither side has enough material to force mate.
    InsufficientMaterial,
    /// 50 full moves without a capture or pawn move.
    FiftyMoveRule,
    ThreefoldRepetition,
}

impl GameStatus {
    pub fn is_over(self) -> bool {
        !matches!(self, GameStatus::InProgress)
    }
}
