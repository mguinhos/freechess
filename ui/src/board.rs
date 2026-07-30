//! The chessboard: the interactive one, and the small previews the lobby
//! renders for every live game.
//!
//! Pieces are Unicode glyphs. The *solid* set (U+265A..U+265F) is used for both
//! colours, with white pieces painted white and outlined in CSS — mixing in the
//! outline glyphs (U+2654..) for white looks inconsistent, because many fonts
//! draw the two sets at different weights and widths.

use chess_core::chess::{Board, Color, Move, PieceKind, Square};
use dioxus::prelude::*;

/// The glyph for a piece kind. Colour is a CSS concern, not a glyph concern.
pub fn glyph(kind: PieceKind) -> &'static str {
    match kind {
        PieceKind::King => "\u{265A}",
        PieceKind::Queen => "\u{265B}",
        PieceKind::Rook => "\u{265C}",
        PieceKind::Bishop => "\u{265D}",
        PieceKind::Knight => "\u{265E}",
        PieceKind::Pawn => "\u{265F}",
    }
}

/// Squares in the order they should be laid out, given the side the board is
/// oriented for.
fn square_order(orientation: Color) -> Vec<Square> {
    let mut out = Vec::with_capacity(64);
    let ranks: Vec<i8> = match orientation {
        Color::White => (0..8).rev().collect(),
        Color::Black => (0..8).collect(),
    };
    for rank in ranks {
        let files: Vec<i8> = match orientation {
            Color::White => (0..8).collect(),
            Color::Black => (0..8).rev().collect(),
        };
        for file in files {
            out.push(Square::from_coords(file, rank).expect("in range"));
        }
    }
    out
}

/// A small, static board — the lobby previews.
#[component]
pub fn MiniBoard(fen: String, orientation: Color) -> Element {
    let board = Board::from_fen(&fen).unwrap_or_default();
    rsx! {
        div { class: "board mini",
            for sq in square_order(orientation) {
                div {
                    key: "{sq.0}",
                    class: if sq.is_light() { "sq light" } else { "sq dark" },
                    if let Some(piece) = board.piece_at(sq) {
                        span {
                            class: if piece.color == Color::White { "piece white" } else { "piece black" },
                            "{glyph(piece.kind)}"
                        }
                    }
                }
            }
        }
    }
}

/// The interactive board.
///
/// `playable` is the colour the viewer may move, or `None` for a spectator —
/// which is what makes spectating safe by construction in the UI, on top of the
/// contract-level rule that only the two players' signatures are accepted.
#[component]
#[allow(clippy::too_many_arguments)]
pub fn GameBoard(
    fen: String,
    orientation: Color,
    playable: Option<Color>,
    last_move: Option<Move>,
    #[props(default = None)] on_move: Option<EventHandler<Move>>,
) -> Element {
    let board = Board::from_fen(&fen).unwrap_or_default();
    let mut selected = use_signal(|| None::<Square>);

    // Destinations for the selected piece, so the UI can show hints and accept
    // a click-to-move. Legality comes from the same engine the contract runs,
    // so the board never offers a move the network would reject.
    let legal_targets: Vec<Move> = match selected() {
        Some(from) => board.legal_moves_from(from),
        None => Vec::new(),
    };

    let king_in_check = if board.is_check() {
        board.king_square(board.side_to_move)
    } else {
        None
    };

    // Precomputed rather than a closure over `board`: the click handlers below
    // outlive this function body, so they cannot borrow it.
    let movable: [bool; 64] =
        std::array::from_fn(|i| match (playable, board.piece_at(Square(i as u8))) {
            (Some(color), Some(piece)) => piece.color == color && board.side_to_move == color,
            _ => false,
        });

    rsx! {
        div { class: "board",
            for sq in square_order(orientation) {
                {
                    let target = legal_targets.iter().find(|m| m.to == sq).copied();
                    let piece = board.piece_at(sq);
                    let is_capture_hint = target.is_some() && piece.is_some();
                    let mut class = String::from(if sq.is_light() { "sq light" } else { "sq dark" });
                    if selected() == Some(sq) { class.push_str(" sel"); }
                    if let Some(mv) = last_move {
                        if mv.from == sq || mv.to == sq { class.push_str(" last"); }
                    }
                    if king_in_check == Some(sq) { class.push_str(" check"); }

                    rsx! {
                        div {
                            key: "{sq.0}",
                            class: "{class}",
                            onclick: move |_| {
                                // Clicking a legal destination plays the move;
                                // otherwise the click (re)selects a piece.
                                if let Some(mv) = target {
                                    if let Some(handler) = &on_move {
                                        handler.call(mv);
                                    }
                                    selected.set(None);
                                } else if movable[sq.0 as usize] {
                                    selected.set(if selected() == Some(sq) { None } else { Some(sq) });
                                } else {
                                    selected.set(None);
                                }
                            },
                            if target.is_some() {
                                span { class: if is_capture_hint { "hint capture" } else { "hint" } }
                            }
                            if let Some(piece) = piece {
                                span {
                                    class: if piece.color == Color::White { "piece white" } else { "piece black" },
                                    "{glyph(piece.kind)}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Material captured by each side, derived from what is missing on the board
/// rather than tracked separately — so it cannot drift out of sync.
pub fn captured_material(board: &Board) -> (String, String) {
    use std::collections::BTreeMap;
    let full: BTreeMap<PieceKind, u32> = [
        (PieceKind::Queen, 1),
        (PieceKind::Rook, 2),
        (PieceKind::Bishop, 2),
        (PieceKind::Knight, 2),
        (PieceKind::Pawn, 8),
    ]
    .into_iter()
    .collect();

    let mut render = |color: Color| -> String {
        let mut out = String::new();
        for (kind, expected) in &full {
            let present = (0..64u8)
                .filter(|i| {
                    board.piece_at(Square(*i)) == Some(chess_core::chess::Piece::new(color, *kind))
                })
                .count() as u32;
            for _ in present..*expected {
                out.push_str(glyph(*kind));
            }
        }
        out
    };

    // What White captured is Black's missing material, and vice versa.
    (render(Color::Black), render(Color::White))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_piece_kind_has_a_distinct_glyph() {
        let kinds = [
            PieceKind::King,
            PieceKind::Queen,
            PieceKind::Rook,
            PieceKind::Bishop,
            PieceKind::Knight,
            PieceKind::Pawn,
        ];
        let mut seen = std::collections::HashSet::new();
        for kind in kinds {
            assert!(seen.insert(glyph(kind)), "duplicate glyph for {kind:?}");
        }
    }

    #[test]
    fn board_orientation_flips_the_layout() {
        let white = square_order(Color::White);
        let black = square_order(Color::Black);
        assert_eq!(white.len(), 64);
        assert_eq!(black.len(), 64);
        // White sees a8 first and h1 last; Black sees the reverse.
        assert_eq!(white.first(), Square::from_algebraic("a8").as_ref());
        assert_eq!(white.last(), Square::from_algebraic("h1").as_ref());
        assert_eq!(black.first(), Square::from_algebraic("h1").as_ref());
        assert_eq!(black.last(), Square::from_algebraic("a8").as_ref());
    }

    #[test]
    fn captured_material_is_empty_at_the_start() {
        let (white, black) = captured_material(&Board::starting_position());
        assert!(white.is_empty() && black.is_empty());
    }

    #[test]
    fn captured_material_reflects_missing_pieces() {
        // Black is missing a queen and a pawn.
        let board = Board::from_fen("rnb1kbnr/1ppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1")
            .expect("valid fen");
        let (by_white, by_black) = captured_material(&board);
        assert!(by_white.contains(glyph(PieceKind::Queen)));
        assert!(by_white.contains(glyph(PieceKind::Pawn)));
        assert!(by_black.is_empty());
    }
}
