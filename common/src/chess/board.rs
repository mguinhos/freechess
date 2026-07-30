//! Board representation, legal move generation and move application.
//!
//! Simple 64-square mailbox rather than bitboards: the whole engine runs inside
//! a contract WASM validating a handful of moves per update, so clarity and
//! small code size matter far more than nodes-per-second.

use super::types::{CastlingRights, Color, GameStatus, Move, Piece, PieceKind, Square};
use serde::{Deserialize, Serialize};

/// Why a move was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveError {
    /// No piece on the origin square.
    EmptySquare,
    /// The piece on the origin square belongs to the other player.
    WrongColor,
    /// Not reachable, or it would leave/put own king in check.
    Illegal,
    /// A pawn reached the last rank without naming a promotion piece, or a
    /// non-promoting move carried one.
    BadPromotion,
    /// The game already ended; no further moves are accepted.
    GameOver,
}

impl core::fmt::Display for MoveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            MoveError::EmptySquare => "no piece on the origin square",
            MoveError::WrongColor => "that piece belongs to the opponent",
            MoveError::Illegal => "illegal move",
            MoveError::BadPromotion => "invalid promotion",
            MoveError::GameOver => "the game is already over",
        };
        write!(f, "{s}")
    }
}

/// What actually happened when a move was applied — everything the UI needs to
/// animate, play a sound and render SAN, all derived rather than trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveOutcome {
    pub mv: Move,
    pub piece: Piece,
    pub captured: Option<Piece>,
    pub is_castle_kingside: bool,
    pub is_castle_queenside: bool,
    pub is_en_passant: bool,
    /// The move leaves the opponent in check.
    pub gives_check: bool,
    /// The move ends the game (mate/stalemate/material).
    pub gives_checkmate: bool,
    /// Standard algebraic notation for this move, e.g. `Nxf7+`.
    pub san: String,
}

const KNIGHT_OFFSETS: [(i8, i8); 8] = [
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
];

const KING_OFFSETS: [(i8, i8); 8] = [
    (0, 1),
    (1, 1),
    (1, 0),
    (1, -1),
    (0, -1),
    (-1, -1),
    (-1, 0),
    (-1, 1),
];

const ROOK_DIRS: [(i8, i8); 4] = [(0, 1), (1, 0), (0, -1), (-1, 0)];
const BISHOP_DIRS: [(i8, i8); 4] = [(1, 1), (1, -1), (-1, -1), (-1, 1)];

/// A full chess position.
///
/// Serialized as its FEN string rather than field-by-field: it keeps the wire
/// format compact and human-readable, and sidesteps serde's lack of impls for
/// 64-element arrays. Parsing validates on the way back in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Board {
    squares: [Option<Piece>; 64],
    pub side_to_move: Color,
    pub castling: CastlingRights,
    /// The square a pawn may be captured on en passant, set only for the single
    /// move immediately after a double pawn push.
    pub en_passant: Option<Square>,
    /// Plies since the last capture or pawn move (the 50-move rule counts to 100).
    pub halfmove_clock: u32,
    pub fullmove_number: u32,
}

impl Default for Board {
    fn default() -> Self {
        Board::starting_position()
    }
}

impl Board {
    /// The standard opening setup.
    pub fn starting_position() -> Board {
        let mut squares = [None; 64];
        let back = [
            PieceKind::Rook,
            PieceKind::Knight,
            PieceKind::Bishop,
            PieceKind::Queen,
            PieceKind::King,
            PieceKind::Bishop,
            PieceKind::Knight,
            PieceKind::Rook,
        ];
        for (file, kind) in back.iter().enumerate() {
            squares[file] = Some(Piece::new(Color::White, *kind));
            squares[56 + file] = Some(Piece::new(Color::Black, *kind));
            squares[8 + file] = Some(Piece::new(Color::White, PieceKind::Pawn));
            squares[48 + file] = Some(Piece::new(Color::Black, PieceKind::Pawn));
        }
        Board {
            squares,
            side_to_move: Color::White,
            castling: CastlingRights::default(),
            en_passant: None,
            halfmove_clock: 0,
            fullmove_number: 1,
        }
    }

    /// A board with no pieces at all — only useful as a base for FEN parsing
    /// and tests.
    pub fn empty() -> Board {
        Board {
            squares: [None; 64],
            side_to_move: Color::White,
            castling: CastlingRights::none(),
            en_passant: None,
            halfmove_clock: 0,
            fullmove_number: 1,
        }
    }

    pub fn piece_at(&self, sq: Square) -> Option<Piece> {
        self.squares[sq.0 as usize]
    }

    pub fn set_piece(&mut self, sq: Square, piece: Option<Piece>) {
        self.squares[sq.0 as usize] = piece;
    }

    pub fn squares(&self) -> &[Option<Piece>; 64] {
        &self.squares
    }

    pub fn king_square(&self, color: Color) -> Option<Square> {
        (0..64u8).map(Square).find(|sq| {
            self.piece_at(*sq)
                == Some(Piece {
                    color,
                    kind: PieceKind::King,
                })
        })
    }

    /// Is `sq` attacked by any piece of `by`? Used both for check detection and
    /// for the squares a king passes through when castling.
    pub fn is_square_attacked(&self, sq: Square, by: Color) -> bool {
        let file = sq.file() as i8;
        let rank = sq.rank() as i8;

        // Pawns: a pawn of `by` attacks `sq` if it sits one rank *behind* the
        // square relative to its own direction of travel.
        let pawn_rank = rank - by.pawn_direction();
        for df in [-1i8, 1] {
            if let Some(p) = Square::from_coords(file + df, pawn_rank) {
                if self.piece_at(p)
                    == Some(Piece {
                        color: by,
                        kind: PieceKind::Pawn,
                    })
                {
                    return true;
                }
            }
        }

        for (df, dr) in KNIGHT_OFFSETS {
            if let Some(p) = Square::from_coords(file + df, rank + dr) {
                if self.piece_at(p)
                    == Some(Piece {
                        color: by,
                        kind: PieceKind::Knight,
                    })
                {
                    return true;
                }
            }
        }

        for (df, dr) in KING_OFFSETS {
            if let Some(p) = Square::from_coords(file + df, rank + dr) {
                if self.piece_at(p)
                    == Some(Piece {
                        color: by,
                        kind: PieceKind::King,
                    })
                {
                    return true;
                }
            }
        }

        // Sliding pieces: walk outward until something blocks.
        let sliders: [(&[(i8, i8)], PieceKind); 2] = [
            (&ROOK_DIRS, PieceKind::Rook),
            (&BISHOP_DIRS, PieceKind::Bishop),
        ];
        for (dirs, kind) in sliders {
            for (df, dr) in dirs {
                let mut f = file + df;
                let mut r = rank + dr;
                while let Some(p) = Square::from_coords(f, r) {
                    match self.piece_at(p) {
                        None => {
                            f += df;
                            r += dr;
                        }
                        Some(piece) => {
                            if piece.color == by
                                && (piece.kind == kind || piece.kind == PieceKind::Queen)
                            {
                                return true;
                            }
                            break;
                        }
                    }
                }
            }
        }

        false
    }

    /// Is the side to move in check?
    pub fn is_check(&self) -> bool {
        self.is_check_for(self.side_to_move)
    }

    pub fn is_check_for(&self, color: Color) -> bool {
        match self.king_square(color) {
            Some(king) => self.is_square_attacked(king, color.opposite()),
            // No king on the board (only reachable in constructed test
            // positions) — treat as not in check rather than panicking.
            None => false,
        }
    }

    /// Every move the side to move may legally play.
    pub fn legal_moves(&self) -> Vec<Move> {
        self.pseudo_legal_moves()
            .into_iter()
            .filter(|mv| self.is_legal(*mv))
            .collect()
    }

    /// Legal moves originating from a single square — what the UI needs to
    /// highlight destinations when a piece is picked up.
    pub fn legal_moves_from(&self, from: Square) -> Vec<Move> {
        self.legal_moves()
            .into_iter()
            .filter(|mv| mv.from == from)
            .collect()
    }

    /// Would this pseudo-legal move leave our own king attacked?
    fn is_legal(&self, mv: Move) -> bool {
        let mover = match self.piece_at(mv.from) {
            Some(p) => p,
            None => return false,
        };
        let mut probe = self.clone();
        probe.apply_unchecked(mv);
        !probe.is_check_for(mover.color)
    }

    /// Moves that follow piece movement rules but may still expose the king.
    /// Castling is filtered for attacked squares here because those conditions
    /// are not expressible as "does the king end up in check".
    pub fn pseudo_legal_moves(&self) -> Vec<Move> {
        let mut moves = Vec::new();
        let me = self.side_to_move;

        for idx in 0..64u8 {
            let from = Square(idx);
            let piece = match self.piece_at(from) {
                Some(p) if p.color == me => p,
                _ => continue,
            };
            let file = from.file() as i8;
            let rank = from.rank() as i8;

            match piece.kind {
                PieceKind::Pawn => self.gen_pawn_moves(from, me, &mut moves),
                PieceKind::Knight => {
                    for (df, dr) in KNIGHT_OFFSETS {
                        if let Some(to) = Square::from_coords(file + df, rank + dr) {
                            if self.piece_at(to).map(|p| p.color) != Some(me) {
                                moves.push(Move::new(from, to));
                            }
                        }
                    }
                }
                PieceKind::King => {
                    for (df, dr) in KING_OFFSETS {
                        if let Some(to) = Square::from_coords(file + df, rank + dr) {
                            if self.piece_at(to).map(|p| p.color) != Some(me) {
                                moves.push(Move::new(from, to));
                            }
                        }
                    }
                    self.gen_castling(me, &mut moves);
                }
                PieceKind::Bishop => self.gen_sliding(from, me, &BISHOP_DIRS, &mut moves),
                PieceKind::Rook => self.gen_sliding(from, me, &ROOK_DIRS, &mut moves),
                PieceKind::Queen => {
                    self.gen_sliding(from, me, &BISHOP_DIRS, &mut moves);
                    self.gen_sliding(from, me, &ROOK_DIRS, &mut moves);
                }
            }
        }

        moves
    }

    fn gen_sliding(&self, from: Square, me: Color, dirs: &[(i8, i8)], moves: &mut Vec<Move>) {
        let file = from.file() as i8;
        let rank = from.rank() as i8;
        for (df, dr) in dirs {
            let mut f = file + df;
            let mut r = rank + dr;
            while let Some(to) = Square::from_coords(f, r) {
                match self.piece_at(to) {
                    None => {
                        moves.push(Move::new(from, to));
                        f += df;
                        r += dr;
                    }
                    Some(p) => {
                        if p.color != me {
                            moves.push(Move::new(from, to));
                        }
                        break;
                    }
                }
            }
        }
    }

    fn gen_pawn_moves(&self, from: Square, me: Color, moves: &mut Vec<Move>) {
        let file = from.file() as i8;
        let rank = from.rank() as i8;
        let dir = me.pawn_direction();
        let promo_rank = me.promotion_rank() as i8;

        let push_with_promotions = |moves: &mut Vec<Move>, to: Square| {
            if to.rank() as i8 == promo_rank {
                for kind in [
                    PieceKind::Queen,
                    PieceKind::Rook,
                    PieceKind::Bishop,
                    PieceKind::Knight,
                ] {
                    moves.push(Move::promoting(from, to, kind));
                }
            } else {
                moves.push(Move::new(from, to));
            }
        };

        // Single push, and the double push only from the starting rank and only
        // when both intervening squares are empty.
        if let Some(one) = Square::from_coords(file, rank + dir) {
            if self.piece_at(one).is_none() {
                push_with_promotions(moves, one);
                if rank == me.pawn_start_rank() as i8 {
                    if let Some(two) = Square::from_coords(file, rank + 2 * dir) {
                        if self.piece_at(two).is_none() {
                            moves.push(Move::new(from, two));
                        }
                    }
                }
            }
        }

        // Captures, including en passant.
        for df in [-1i8, 1] {
            if let Some(to) = Square::from_coords(file + df, rank + dir) {
                let is_normal_capture = self.piece_at(to).map(|p| p.color) == Some(me.opposite());
                let is_en_passant = self.en_passant == Some(to);
                if is_normal_capture || is_en_passant {
                    push_with_promotions(moves, to);
                }
            }
        }
    }

    fn gen_castling(&self, me: Color, moves: &mut Vec<Move>) {
        let back = me.back_rank() as i8;
        let king_from = match Square::from_coords(4, back) {
            Some(s) => s,
            None => return,
        };
        // The rights flags are only meaningful if the king is actually home.
        if self.piece_at(king_from)
            != Some(Piece {
                color: me,
                kind: PieceKind::King,
            })
        {
            return;
        }
        // Castling out of check is never allowed.
        if self.is_square_attacked(king_from, me.opposite()) {
            return;
        }

        let rook_of = |file: i8| -> bool {
            Square::from_coords(file, back).and_then(|s| self.piece_at(s))
                == Some(Piece {
                    color: me,
                    kind: PieceKind::Rook,
                })
        };
        let empty = |file: i8| -> bool {
            Square::from_coords(file, back)
                .map(|s| self.piece_at(s).is_none())
                .unwrap_or(false)
        };
        let safe = |file: i8| -> bool {
            Square::from_coords(file, back)
                .map(|s| !self.is_square_attacked(s, me.opposite()))
                .unwrap_or(false)
        };

        // Kingside: f and g empty, king passes f and lands on g.
        if self.castling.kingside(me) && rook_of(7) && empty(5) && empty(6) && safe(5) && safe(6) {
            if let Some(to) = Square::from_coords(6, back) {
                moves.push(Move::new(king_from, to));
            }
        }
        // Queenside: b, c and d empty; the king only passes d and lands on c,
        // so b may be attacked.
        if self.castling.queenside(me)
            && rook_of(0)
            && empty(1)
            && empty(2)
            && empty(3)
            && safe(3)
            && safe(2)
        {
            if let Some(to) = Square::from_coords(2, back) {
                moves.push(Move::new(king_from, to));
            }
        }
    }

    /// Validate `mv` against the current position and apply it, returning
    /// everything derived about it (SAN, capture, check).
    ///
    /// This is the only entry point the game state uses, so an illegal move can
    /// never enter a game's move list — on any node.
    pub fn make_move(&mut self, mv: Move) -> Result<MoveOutcome, MoveError> {
        let piece = self.piece_at(mv.from).ok_or(MoveError::EmptySquare)?;
        if piece.color != self.side_to_move {
            return Err(MoveError::WrongColor);
        }

        // A pawn landing on the last rank must name a promotion piece, and
        // nothing else may carry one. Checked before legality so the caller
        // gets the more specific error.
        let is_promotion_move =
            piece.kind == PieceKind::Pawn && mv.to.rank() == self.side_to_move.promotion_rank();
        match (is_promotion_move, mv.promotion) {
            (true, None) => return Err(MoveError::BadPromotion),
            (false, Some(_)) => return Err(MoveError::BadPromotion),
            (true, Some(k))
                if !matches!(
                    k,
                    PieceKind::Queen | PieceKind::Rook | PieceKind::Bishop | PieceKind::Knight
                ) =>
            {
                return Err(MoveError::BadPromotion)
            }
            _ => {}
        }

        if !self.legal_moves().contains(&mv) {
            return Err(MoveError::Illegal);
        }

        // SAN needs the pre-move position for disambiguation, so build it here
        // and patch in the check/mate suffix after applying.
        let san_body = self.san_body(mv, piece);

        let captured = self.captured_piece(mv, piece);
        let is_en_passant = piece.kind == PieceKind::Pawn
            && Some(mv.to) == self.en_passant
            && self.piece_at(mv.to).is_none();
        let file_delta = mv.to.file() as i8 - mv.from.file() as i8;
        let is_castle_kingside = piece.kind == PieceKind::King && file_delta == 2;
        let is_castle_queenside = piece.kind == PieceKind::King && file_delta == -2;

        self.apply_unchecked(mv);

        let gives_check = self.is_check();
        let opponent_has_moves = !self.legal_moves().is_empty();
        let gives_checkmate = gives_check && !opponent_has_moves;

        let suffix = if gives_checkmate {
            "#"
        } else if gives_check {
            "+"
        } else {
            ""
        };
        let san = format!("{san_body}{suffix}");

        Ok(MoveOutcome {
            mv,
            piece,
            captured,
            is_castle_kingside,
            is_castle_queenside,
            is_en_passant,
            gives_check,
            gives_checkmate,
            san,
        })
    }

    fn captured_piece(&self, mv: Move, piece: Piece) -> Option<Piece> {
        if let Some(p) = self.piece_at(mv.to) {
            return Some(p);
        }
        // En passant: the captured pawn sits beside the destination, not on it.
        if piece.kind == PieceKind::Pawn && Some(mv.to) == self.en_passant {
            let captured_sq = Square::from_coords(
                mv.to.file() as i8,
                mv.to.rank() as i8 - piece.color.pawn_direction(),
            )?;
            return self.piece_at(captured_sq);
        }
        None
    }

    /// Apply a move that is already known to be pseudo-legal, with no
    /// validation. Used by legality probing and by `make_move`.
    fn apply_unchecked(&mut self, mv: Move) {
        let piece = match self.piece_at(mv.from) {
            Some(p) => p,
            None => return,
        };
        let me = piece.color;
        let is_capture = self.piece_at(mv.to).is_some();
        let is_en_passant = piece.kind == PieceKind::Pawn
            && Some(mv.to) == self.en_passant
            && self.piece_at(mv.to).is_none();

        // Remove the en-passant victim, which is not on the destination square.
        if is_en_passant {
            if let Some(victim) =
                Square::from_coords(mv.to.file() as i8, mv.to.rank() as i8 - me.pawn_direction())
            {
                self.set_piece(victim, None);
            }
        }

        // Move the rook too when castling.
        if piece.kind == PieceKind::King {
            let back = me.back_rank() as i8;
            let delta = mv.to.file() as i8 - mv.from.file() as i8;
            if delta == 2 {
                if let (Some(rf), Some(rt)) =
                    (Square::from_coords(7, back), Square::from_coords(5, back))
                {
                    let rook = self.piece_at(rf);
                    self.set_piece(rf, None);
                    self.set_piece(rt, rook);
                }
            } else if delta == -2 {
                if let (Some(rf), Some(rt)) =
                    (Square::from_coords(0, back), Square::from_coords(3, back))
                {
                    let rook = self.piece_at(rf);
                    self.set_piece(rf, None);
                    self.set_piece(rt, rook);
                }
            }
        }

        let landing = match mv.promotion {
            Some(kind) => Piece::new(me, kind),
            None => piece,
        };
        self.set_piece(mv.from, None);
        self.set_piece(mv.to, Some(landing));

        // Castling rights: lost when the king or a rook moves, and when a rook
        // is captured on its home square (easy to forget — a rook captured on
        // a1 must clear White's queenside right even though White never moved).
        if piece.kind == PieceKind::King {
            self.castling.clear_for(me);
        }
        self.revoke_rook_rights(mv.from);
        self.revoke_rook_rights(mv.to);

        // En passant target: set only by a double pawn push.
        self.en_passant = if piece.kind == PieceKind::Pawn
            && (mv.to.rank() as i8 - mv.from.rank() as i8).abs() == 2
        {
            Square::from_coords(
                mv.from.file() as i8,
                mv.from.rank() as i8 + me.pawn_direction(),
            )
        } else {
            None
        };

        if piece.kind == PieceKind::Pawn || is_capture || is_en_passant {
            self.halfmove_clock = 0;
        } else {
            self.halfmove_clock += 1;
        }
        if me == Color::Black {
            self.fullmove_number += 1;
        }
        self.side_to_move = me.opposite();
    }

    /// Clear the castling right tied to a rook home square when a piece leaves
    /// or lands on it.
    fn revoke_rook_rights(&mut self, sq: Square) {
        match (sq.file(), sq.rank()) {
            (0, 0) => self.castling.white_queenside = false,
            (7, 0) => self.castling.white_kingside = false,
            (0, 7) => self.castling.black_queenside = false,
            (7, 7) => self.castling.black_kingside = false,
            _ => {}
        }
    }

    /// SAN without the check/mate suffix, computed against the pre-move board.
    fn san_body(&self, mv: Move, piece: Piece) -> String {
        let file_delta = mv.to.file() as i8 - mv.from.file() as i8;
        if piece.kind == PieceKind::King && file_delta == 2 {
            return "O-O".to_string();
        }
        if piece.kind == PieceKind::King && file_delta == -2 {
            return "O-O-O".to_string();
        }

        let is_capture = self.piece_at(mv.to).is_some()
            || (piece.kind == PieceKind::Pawn && Some(mv.to) == self.en_passant);

        let mut san = String::new();
        if piece.kind == PieceKind::Pawn {
            if is_capture {
                san.push((b'a' + mv.from.file()) as char);
                san.push('x');
            }
        } else {
            san.push(piece.kind.letter().to_ascii_uppercase());
            san.push_str(&self.disambiguation(mv, piece));
            if is_capture {
                san.push('x');
            }
        }
        san.push_str(&mv.to.to_string());
        if let Some(kind) = mv.promotion {
            san.push('=');
            san.push(kind.letter().to_ascii_uppercase());
        }
        san
    }

    /// The minimal file/rank qualifier needed when more than one piece of the
    /// same kind can reach the destination.
    fn disambiguation(&self, mv: Move, piece: Piece) -> String {
        let rivals: Vec<Move> = self
            .legal_moves()
            .into_iter()
            .filter(|m| {
                m.to == mv.to
                    && m.from != mv.from
                    && self.piece_at(m.from) == Some(piece)
                    // Promotion variants of the same move are not rivals.
                    && m.promotion == mv.promotion
            })
            .collect();

        if rivals.is_empty() {
            return String::new();
        }
        // File alone is enough when no rival shares our file.
        if !rivals.iter().any(|m| m.from.file() == mv.from.file()) {
            return ((b'a' + mv.from.file()) as char).to_string();
        }
        // Otherwise try rank alone, then fall back to the full square.
        if !rivals.iter().any(|m| m.from.rank() == mv.from.rank()) {
            return ((b'1' + mv.from.rank()) as char).to_string();
        }
        mv.from.to_string()
    }

    /// Neither side can force mate: K vs K, K+minor vs K, and K+B vs K+B with
    /// both bishops on the same color complex.
    pub fn has_insufficient_material(&self) -> bool {
        let mut minors: Vec<(Color, PieceKind, Square)> = Vec::new();
        for idx in 0..64u8 {
            let sq = Square(idx);
            if let Some(p) = self.piece_at(sq) {
                match p.kind {
                    PieceKind::King => {}
                    PieceKind::Bishop | PieceKind::Knight => minors.push((p.color, p.kind, sq)),
                    // A pawn, rook or queen anywhere means mate is still
                    // reachable in principle.
                    _ => return false,
                }
            }
        }
        match minors.len() {
            0 | 1 => true,
            2 => {
                let (c0, k0, s0) = minors[0];
                let (c1, k1, s1) = minors[1];
                k0 == PieceKind::Bishop
                    && k1 == PieceKind::Bishop
                    && c0 != c1
                    && s0.is_light() == s1.is_light()
            }
            _ => false,
        }
    }

    /// Position identity for threefold repetition: placement, side to move,
    /// castling rights and a *relevant* en-passant square.
    ///
    /// The en-passant square only counts when an enemy pawn is actually beside
    /// the pushed pawn — otherwise two positions that play identically would
    /// hash differently and a real repetition would be missed.
    pub fn position_key(&self) -> String {
        let mut key = String::with_capacity(80);
        for rank in (0..8).rev() {
            for file in 0..8 {
                let sq = Square::from_coords(file, rank).expect("in range");
                match self.piece_at(sq) {
                    Some(p) => key.push(p.fen_char()),
                    None => key.push('.'),
                }
            }
        }
        key.push(match self.side_to_move {
            Color::White => 'w',
            Color::Black => 'b',
        });
        key.push_str(&self.castling.to_fen());
        if let Some(ep) = self.en_passant {
            if self.en_passant_is_capturable(ep) {
                key.push_str(&ep.to_string());
            }
        }
        key
    }

    fn en_passant_is_capturable(&self, ep: Square) -> bool {
        let me = self.side_to_move;
        let from_rank = ep.rank() as i8 - me.pawn_direction();
        [-1i8, 1].iter().any(|df| {
            Square::from_coords(ep.file() as i8 + df, from_rank).and_then(|s| self.piece_at(s))
                == Some(Piece {
                    color: me,
                    kind: PieceKind::Pawn,
                })
        })
    }

    /// Game status for this position. `repetitions` is how many times this
    /// exact position has already occurred in the game (including now), which
    /// the board alone cannot know — [`Game`](super::Game) supplies it.
    pub fn status(&self, repetitions: u32) -> GameStatus {
        if self.legal_moves().is_empty() {
            return if self.is_check() {
                GameStatus::Checkmate {
                    winner: self.side_to_move.opposite(),
                }
            } else {
                GameStatus::Stalemate
            };
        }
        if self.has_insufficient_material() {
            return GameStatus::InsufficientMaterial;
        }
        if repetitions >= 3 {
            return GameStatus::ThreefoldRepetition;
        }
        if self.halfmove_clock >= 100 {
            return GameStatus::FiftyMoveRule;
        }
        GameStatus::InProgress
    }
}
