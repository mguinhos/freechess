//! Engine tests.
//!
//! The centrepiece is `perft`: counting leaf nodes of the move tree to a fixed
//! depth against published reference values. It is the standard way to prove a
//! move generator handles castling, en passant, promotion and pin/check
//! interactions correctly — a single wrong edge case shifts the count.

use super::*;
use crate::chess::types::GameStatus;

/// Count leaf nodes of the legal move tree at `depth`.
fn perft(board: &Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let moves = board.legal_moves();
    if depth == 1 {
        return moves.len() as u64;
    }
    let mut nodes = 0;
    for mv in moves {
        let mut next = board.clone();
        next.make_move(mv).expect("generated move must be legal");
        nodes += perft(&next, depth - 1);
    }
    nodes
}

fn board_from(fen: &str) -> Board {
    Board::from_fen(fen).expect("test FEN parses")
}

// The five standard perft positions (chessprogramming.org). Depths kept
// modest so the suite stays fast in debug builds; `perft_deep` covers more.

#[test]
fn perft_starting_position() {
    let b = Board::starting_position();
    assert_eq!(perft(&b, 1), 20);
    assert_eq!(perft(&b, 2), 400);
    assert_eq!(perft(&b, 3), 8_902);
}

#[test]
fn perft_kiwipete() {
    // Dense middlegame: castling both sides, pins, en passant available.
    let b = board_from("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1");
    assert_eq!(perft(&b, 1), 48);
    assert_eq!(perft(&b, 2), 2_039);
    assert_eq!(perft(&b, 3), 97_862);
}

#[test]
fn perft_position_3() {
    // Sparse endgame that catches en-passant-into-check bugs.
    let b = board_from("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1");
    assert_eq!(perft(&b, 1), 14);
    assert_eq!(perft(&b, 2), 191);
    assert_eq!(perft(&b, 3), 2_812);
    assert_eq!(perft(&b, 4), 43_238);
}

#[test]
fn perft_position_4() {
    // Promotion-heavy, and the mirrored version catches color asymmetries.
    let b = board_from("r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1");
    assert_eq!(perft(&b, 1), 6);
    assert_eq!(perft(&b, 2), 264);
    assert_eq!(perft(&b, 3), 9_467);

    let mirrored = board_from("r2q1rk1/pP1p2pp/Q4n2/bbp1p3/Np6/1B3NBn/pPPP1PPP/R3K2R b KQ - 0 1");
    assert_eq!(perft(&mirrored, 1), 6);
    assert_eq!(perft(&mirrored, 2), 264);
    assert_eq!(perft(&mirrored, 3), 9_467);
}

#[test]
fn perft_position_5() {
    let b = board_from("rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8");
    assert_eq!(perft(&b, 1), 44);
    assert_eq!(perft(&b, 2), 1_486);
    assert_eq!(perft(&b, 3), 62_379);
}

/// Deeper counts. Slow in debug (`cargo test --release -- --ignored`).
#[test]
#[ignore = "slow: run with --release"]
fn perft_deep() {
    assert_eq!(perft(&Board::starting_position(), 5), 4_865_609);
    let kiwipete =
        board_from("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1");
    assert_eq!(perft(&kiwipete, 4), 4_085_603);
    let p3 = board_from("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1");
    assert_eq!(perft(&p3, 5), 674_624);
}

#[test]
fn fen_roundtrips() {
    for fen in [
        STARTING_FEN,
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        "4k3/8/8/8/8/8/4P3/4K3 w - - 5 39",
    ] {
        let board = board_from(fen);
        assert_eq!(board.to_fen(), fen, "roundtrip failed for {fen}");
    }
}

#[test]
fn rejects_malformed_fen() {
    for bad in [
        "not a fen",
        // Only 7 ranks.
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP w KQkq - 0 1",
        // Rank describing 9 files.
        "rnbqkbnrr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        // Bad side to move.
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR x KQkq - 0 1",
    ] {
        assert!(Board::from_fen(bad).is_err(), "should reject {bad}");
    }
}

#[test]
fn scholars_mate_is_checkmate() {
    let mut game = Game::new();
    for uci in ["e2e4", "e7e5", "f1c4", "b8c6", "d1h5", "g8f6", "h5f7"] {
        game.play(Move::from_uci(uci).unwrap())
            .unwrap_or_else(|e| panic!("{uci} rejected: {e}"));
    }
    assert_eq!(
        game.status(),
        GameStatus::Checkmate {
            winner: Color::White
        }
    );
    assert_eq!(game.san_list().last().unwrap(), "Qxf7#");
    // No further moves may be recorded once the game is decided.
    assert_eq!(
        game.play(Move::from_uci("e8f7").unwrap()),
        Err(MoveError::GameOver)
    );
}

#[test]
fn fools_mate_is_checkmate() {
    let mut game = Game::new();
    for uci in ["f2f3", "e7e5", "g2g4", "d8h4"] {
        game.play(Move::from_uci(uci).unwrap()).unwrap();
    }
    assert_eq!(
        game.status(),
        GameStatus::Checkmate {
            winner: Color::Black
        }
    );
    assert_eq!(game.san_list(), vec!["f3", "e5", "g4", "Qh4#"]);
}

#[test]
fn stalemate_is_detected() {
    // Black king on a8, white queen c7, white king somewhere far: black to move
    // has no legal move but is not in check.
    let board = board_from("k7/2Q5/8/8/8/8/8/4K3 b - - 0 1");
    assert!(!board.is_check());
    assert!(board.legal_moves().is_empty());
    assert_eq!(board.status(1), GameStatus::Stalemate);
}

#[test]
fn castling_moves_the_rook_and_clears_rights() {
    let mut board = board_from("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
    let outcome = board.make_move(Move::from_uci("e1g1").unwrap()).unwrap();
    assert!(outcome.is_castle_kingside);
    assert_eq!(outcome.san, "O-O");
    assert_eq!(
        board.piece_at(Square::from_algebraic("f1").unwrap()),
        Some(Piece::new(Color::White, PieceKind::Rook))
    );
    assert_eq!(board.piece_at(Square::from_algebraic("h1").unwrap()), None);
    assert!(!board.castling.white_kingside && !board.castling.white_queenside);
    // Black still has both rights.
    assert!(board.castling.black_kingside && board.castling.black_queenside);

    let mut board = board_from("r3k2r/8/8/8/8/8/8/R3K2R b kq - 0 1");
    let outcome = board.make_move(Move::from_uci("e8c8").unwrap()).unwrap();
    assert!(outcome.is_castle_queenside);
    assert_eq!(outcome.san, "O-O-O");
    assert_eq!(
        board.piece_at(Square::from_algebraic("d8").unwrap()),
        Some(Piece::new(Color::Black, PieceKind::Rook))
    );
}

#[test]
fn cannot_castle_through_or_out_of_check() {
    // Rook on e8 pins the king's square: castling out of check is illegal.
    let board = board_from("4r3/8/8/8/8/8/8/R3K2R w KQ - 0 1");
    assert!(!board
        .legal_moves()
        .contains(&Move::from_uci("e1g1").unwrap()));

    // Rook on f8 attacks f1, the square the king passes through.
    let board = board_from("5r2/8/8/8/8/8/8/R3K2R w KQ - 0 1");
    assert!(!board
        .legal_moves()
        .contains(&Move::from_uci("e1g1").unwrap()));

    // Rook on b8 attacks b1, which the queenside king does NOT pass through —
    // that castle stays legal.
    let board = board_from("1r6/8/8/8/8/8/8/R3K2R w KQ - 0 1");
    assert!(board
        .legal_moves()
        .contains(&Move::from_uci("e1c1").unwrap()));
}

#[test]
fn capturing_a_rook_on_its_home_square_revokes_castling() {
    // Black bishop takes the h1 rook; White must lose the kingside right even
    // though White never moved a piece.
    let mut board = board_from("4k3/8/8/8/8/8/6b1/R3K2R b KQ - 0 1");
    board.make_move(Move::from_uci("g2h1").unwrap()).unwrap();
    assert!(!board.castling.white_kingside);
    assert!(board.castling.white_queenside);
}

#[test]
fn en_passant_captures_the_passed_pawn() {
    let mut board = board_from("4k3/8/8/8/4p3/8/3P4/4K3 w - - 0 1");
    board.make_move(Move::from_uci("d2d4").unwrap()).unwrap();
    assert_eq!(board.en_passant, Square::from_algebraic("d3"));

    let outcome = board.make_move(Move::from_uci("e4d3").unwrap()).unwrap();
    assert!(outcome.is_en_passant);
    assert_eq!(
        outcome.captured,
        Some(Piece::new(Color::White, PieceKind::Pawn))
    );
    assert_eq!(outcome.san, "exd3");
    // The captured pawn sat on d4, not on the destination square.
    assert_eq!(board.piece_at(Square::from_algebraic("d4").unwrap()), None);
}

#[test]
fn en_passant_expires_after_one_move() {
    let mut board = board_from("4k3/7p/8/8/4p3/8/3P4/4K3 w - - 0 1");
    board.make_move(Move::from_uci("d2d4").unwrap()).unwrap();
    board.make_move(Move::from_uci("h7h6").unwrap()).unwrap();
    assert_eq!(board.en_passant, None);
    assert!(!board
        .legal_moves()
        .contains(&Move::from_uci("e4d3").unwrap()));
}

#[test]
fn promotion_requires_a_piece_and_rejects_king_or_pawn() {
    let mut board = board_from("4k3/P7/8/8/8/8/8/4K3 w - - 0 1");
    assert_eq!(
        board.make_move(Move::new(
            Square::from_algebraic("a7").unwrap(),
            Square::from_algebraic("a8").unwrap()
        )),
        Err(MoveError::BadPromotion)
    );
    for bad in [PieceKind::King, PieceKind::Pawn] {
        assert_eq!(
            board.make_move(Move::promoting(
                Square::from_algebraic("a7").unwrap(),
                Square::from_algebraic("a8").unwrap(),
                bad
            )),
            Err(MoveError::BadPromotion)
        );
    }
    let outcome = board.make_move(Move::from_uci("a7a8q").unwrap()).unwrap();
    assert_eq!(outcome.san, "a8=Q+");
    assert_eq!(
        board.piece_at(Square::from_algebraic("a8").unwrap()),
        Some(Piece::new(Color::White, PieceKind::Queen))
    );
}

#[test]
fn non_promotion_move_rejects_a_promotion_piece() {
    let mut board = Board::starting_position();
    assert_eq!(
        board.make_move(Move::promoting(
            Square::from_algebraic("e2").unwrap(),
            Square::from_algebraic("e4").unwrap(),
            PieceKind::Queen
        )),
        Err(MoveError::BadPromotion)
    );
}

#[test]
fn pinned_piece_cannot_move() {
    // The knight on e2 is pinned to the king on e1 by the rook on e8.
    let board = board_from("4r3/8/8/8/8/8/4N3/4K3 w - - 0 1");
    assert!(!board
        .legal_moves()
        .iter()
        .any(|m| m.from == Square::from_algebraic("e2").unwrap()));
}

#[test]
fn moving_out_of_turn_is_rejected() {
    let mut board = Board::starting_position();
    assert_eq!(
        board.make_move(Move::from_uci("e7e5").unwrap()),
        Err(MoveError::WrongColor)
    );
    assert_eq!(
        board.make_move(Move::from_uci("e3e4").unwrap()),
        Err(MoveError::EmptySquare)
    );
}

#[test]
fn san_disambiguates_by_file_then_rank() {
    // Two knights on b1 and f3 both reach d2 — file distinguishes them.
    let mut board = board_from("4k3/8/8/8/8/5N2/8/1N2K3 w - - 0 1");
    let outcome = board.make_move(Move::from_uci("b1d2").unwrap()).unwrap();
    assert_eq!(outcome.san, "Nbd2");

    // Two rooks on the same file need the rank.
    let mut board = board_from("4k3/8/8/R7/8/8/R7/4K3 w - - 0 1");
    let outcome = board.make_move(Move::from_uci("a2a4").unwrap()).unwrap();
    assert_eq!(outcome.san, "R2a4");

    // Three queens where neither file nor rank alone suffices.
    let mut board = board_from("4k3/8/8/8/Q6Q/8/8/Q3K3 w - - 0 1");
    let outcome = board.make_move(Move::from_uci("a4d4").unwrap()).unwrap();
    assert_eq!(outcome.san, "Qa4d4");
}

#[test]
fn threefold_repetition_is_detected() {
    let mut game = Game::new();
    // Knights shuffle out and back twice, returning to the start position for
    // the third time.
    for uci in [
        "g1f3", "g8f6", "f3g1", "f6g8", "g1f3", "g8f6", "f3g1", "f6g8",
    ] {
        game.play(Move::from_uci(uci).unwrap()).unwrap();
    }
    assert_eq!(game.status(), GameStatus::ThreefoldRepetition);
}

#[test]
fn fifty_move_rule_is_detected() {
    // Halfmove clock one ply short of 100; a quiet move trips it.
    let mut game = Game::from_board(board_from("4k3/8/8/8/8/8/R7/4K3 w - - 99 60"));
    assert_eq!(game.status(), GameStatus::InProgress);
    game.play(Move::from_uci("a2a3").unwrap()).unwrap();
    assert_eq!(game.status(), GameStatus::FiftyMoveRule);
}

#[test]
fn pawn_move_resets_the_halfmove_clock() {
    let mut game = Game::from_board(board_from("4k3/p7/8/8/8/8/R7/4K3 b - - 99 60"));
    game.play(Move::from_uci("a7a6").unwrap()).unwrap();
    assert_eq!(game.status(), GameStatus::InProgress);
    assert_eq!(game.board().halfmove_clock, 0);
}

#[test]
fn insufficient_material_variants() {
    // Bare kings.
    assert!(board_from("4k3/8/8/8/8/8/8/4K3 w - - 0 1").has_insufficient_material());
    // King and knight.
    assert!(board_from("4k3/8/8/8/8/8/8/3NK3 w - - 0 1").has_insufficient_material());
    // King and bishop.
    assert!(board_from("4k3/8/8/8/8/8/8/3BK3 w - - 0 1").has_insufficient_material());
    // Opposite-color bishops on the same square color: still drawn.
    assert!(board_from("2b1k3/8/8/8/8/8/8/3BK3 w - - 0 1").has_insufficient_material());
    // A single pawn can promote, so mate remains possible.
    assert!(!board_from("4k3/8/8/8/8/8/P7/4K3 w - - 0 1").has_insufficient_material());
    // Two knights is not *forced* mate but is conventionally still playable;
    // we follow the FIDE "cannot possibly arise" reading and keep it playable.
    assert!(!board_from("4k3/8/8/8/8/8/8/1N1NK3 w - - 0 1").has_insufficient_material());
}

#[test]
fn move_uci_roundtrips() {
    for uci in ["e2e4", "a7a8q", "h1h8", "e7e8n"] {
        let mv = Move::from_uci(uci).expect("parses");
        assert_eq!(mv.to_uci(), uci);
    }
    assert!(Move::from_uci("e2").is_none());
    assert!(Move::from_uci("e2e4k").is_none());
    assert!(Move::from_uci("z9z9").is_none());
}

#[test]
fn promotion_signing_bytes_do_not_alias() {
    // Every promotion choice must produce distinct signed bytes, otherwise a
    // peer could swap a knight promotion for a queen under the same signature.
    let from = Square::from_algebraic("a7").unwrap();
    let to = Square::from_algebraic("a8").unwrap();
    let mut seen = std::collections::HashSet::new();
    for promo in [
        None,
        Some(PieceKind::Knight),
        Some(PieceKind::Bishop),
        Some(PieceKind::Rook),
        Some(PieceKind::Queen),
        Some(PieceKind::Pawn),
        Some(PieceKind::King),
    ] {
        let mv = Move {
            from,
            to,
            promotion: promo,
        };
        assert!(seen.insert(mv.signing_bytes()), "collision for {promo:?}");
    }
}

#[test]
fn replay_reconstructs_every_intermediate_position() {
    let moves: Vec<Move> = ["e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6"]
        .iter()
        .map(|u| Move::from_uci(u).unwrap())
        .collect();
    let game = Game::from_moves(&moves).expect("legal line");

    assert_eq!(game.ply(), 6);
    assert_eq!(game.fen_at(0), STARTING_FEN);
    // Replaying a prefix must produce the same FEN the full game recorded for
    // that ply — this is what the replay scrubber relies on.
    for ply in 0..=moves.len() {
        let prefix = Game::from_moves(&moves[..ply]).expect("prefix is legal");
        assert_eq!(
            prefix.current_fen(),
            game.fen_at(ply),
            "mismatch at ply {ply}"
        );
    }
}

#[test]
fn replay_rejects_an_illegal_move_and_reports_the_ply() {
    let moves = vec![
        Move::from_uci("e2e4").unwrap(),
        Move::from_uci("e7e5").unwrap(),
        // The queen cannot jump over its own d2 pawn.
        Move::from_uci("d1d5").unwrap(),
    ];
    let err = Game::from_moves(&moves).expect_err("must reject");
    assert_eq!(err, (2, MoveError::Illegal));
}

#[test]
fn pgn_export_has_tags_and_numbered_moves() {
    let game = Game::from_moves(
        &["e2e4", "e7e5", "g1f3"]
            .iter()
            .map(|u| Move::from_uci(u).unwrap())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let pgn = game.to_pgn(
        &[("White", "alice".to_string()), ("Black", "bob".to_string())],
        "*",
    );
    assert!(pgn.contains("[White \"alice\"]"));
    assert!(pgn.contains("[Black \"bob\"]"));
    assert!(pgn.contains("[Result \"*\"]"));
    assert!(pgn.contains("1. e4 e5 2. Nf3"));
}

#[test]
fn pgn_escapes_quotes_in_nicknames() {
    let game = Game::new();
    let pgn = game.to_pgn(&[("White", "he said \"hi\"".to_string())], "*");
    assert!(pgn.contains(r#"[White "he said \"hi\""]"#));
}
