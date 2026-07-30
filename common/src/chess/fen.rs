//! FEN serialization and parsing.
//!
//! Used for the mini-boards on the lobby page (a game summary carries a FEN so
//! the home page can render every live board without replaying move lists) and
//! for setting up test positions.

use super::board::Board;
use super::types::{CastlingRights, Color, Piece, Square};

pub const STARTING_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FenError(pub String);

impl core::fmt::Display for FenError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "invalid FEN: {}", self.0)
    }
}

/// Serde bridge: a `Board` goes over the wire as its FEN string.
impl From<Board> for String {
    fn from(board: Board) -> String {
        board.to_fen()
    }
}

impl TryFrom<String> for Board {
    type Error = FenError;

    fn try_from(fen: String) -> Result<Board, FenError> {
        Board::from_fen(&fen)
    }
}

impl Board {
    /// Full six-field FEN.
    pub fn to_fen(&self) -> String {
        let mut fen = String::with_capacity(90);

        for rank in (0..8i8).rev() {
            let mut empty_run = 0u8;
            for file in 0..8i8 {
                let sq = Square::from_coords(file, rank).expect("in range");
                match self.piece_at(sq) {
                    Some(p) => {
                        if empty_run > 0 {
                            fen.push((b'0' + empty_run) as char);
                            empty_run = 0;
                        }
                        fen.push(p.fen_char());
                    }
                    None => empty_run += 1,
                }
            }
            if empty_run > 0 {
                fen.push((b'0' + empty_run) as char);
            }
            if rank > 0 {
                fen.push('/');
            }
        }

        fen.push(' ');
        fen.push(match self.side_to_move {
            Color::White => 'w',
            Color::Black => 'b',
        });
        fen.push(' ');
        fen.push_str(&self.castling.to_fen());
        fen.push(' ');
        match self.en_passant {
            Some(sq) => fen.push_str(&sq.to_string()),
            None => fen.push('-'),
        }
        fen.push(' ');
        fen.push_str(&self.halfmove_clock.to_string());
        fen.push(' ');
        fen.push_str(&self.fullmove_number.to_string());
        fen
    }

    /// Parse a FEN. The last two (counter) fields are optional, as they are in
    /// most FEN dumps found in the wild.
    pub fn from_fen(fen: &str) -> Result<Board, FenError> {
        let fields: Vec<&str> = fen.split_whitespace().collect();
        if fields.len() < 4 {
            return Err(FenError(format!(
                "expected at least 4 fields, got {}",
                fields.len()
            )));
        }

        let mut board = Board::empty();

        let ranks: Vec<&str> = fields[0].split('/').collect();
        if ranks.len() != 8 {
            return Err(FenError(format!("expected 8 ranks, got {}", ranks.len())));
        }
        for (i, row) in ranks.iter().enumerate() {
            // FEN lists rank 8 first.
            let rank = 7i8 - i as i8;
            let mut file = 0i8;
            for c in row.chars() {
                if let Some(skip) = c.to_digit(10) {
                    if skip == 0 || skip > 8 {
                        return Err(FenError(format!("bad empty-square run '{c}'")));
                    }
                    file += skip as i8;
                } else {
                    let piece = Piece::from_fen_char(c)
                        .ok_or_else(|| FenError(format!("bad piece '{c}'")))?;
                    let sq = Square::from_coords(file, rank)
                        .ok_or_else(|| FenError(format!("rank {} overflows", rank + 1)))?;
                    board.set_piece(sq, Some(piece));
                    file += 1;
                }
            }
            if file != 8 {
                return Err(FenError(format!(
                    "rank {} describes {} files, expected 8",
                    rank + 1,
                    file
                )));
            }
        }

        board.side_to_move = match fields[1] {
            "w" => Color::White,
            "b" => Color::Black,
            other => return Err(FenError(format!("bad side to move '{other}'"))),
        };

        let mut rights = CastlingRights::none();
        if fields[2] != "-" {
            for c in fields[2].chars() {
                match c {
                    'K' => rights.white_kingside = true,
                    'Q' => rights.white_queenside = true,
                    'k' => rights.black_kingside = true,
                    'q' => rights.black_queenside = true,
                    other => return Err(FenError(format!("bad castling flag '{other}'"))),
                }
            }
        }
        board.castling = rights;

        board.en_passant = if fields[3] == "-" {
            None
        } else {
            Some(
                Square::from_algebraic(fields[3])
                    .ok_or_else(|| FenError(format!("bad en passant square '{}'", fields[3])))?,
            )
        };

        board.halfmove_clock = fields.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        board.fullmove_number = fields.get(5).and_then(|s| s.parse().ok()).unwrap_or(1);

        Ok(board)
    }
}
