//! Shared fixtures for the state tests.

use crate::certificate::{CertificateDraft, GameCertificate};
use crate::chess::{Color, Move};
use crate::game::moves::AuthorizedMove;
use crate::game::opponent::{OpponentSlotV1, SignedAcceptance, SignedJoin};
use crate::game::setup::{GameSetup, GameSetupV1, TimeControl};
use crate::game::{ChessGameParametersV1, ChessGameStateV1};
use crate::identity::GameId;
use ed25519_dalek::SigningKey;
use freenet_scaffold::ComposableState;
use rand::rngs::OsRng;

pub const T0: i64 = 1_700_000_000_000;

pub fn key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

/// A game created by `creator` (playing White) at `created_at`, nobody joined.
pub fn open_game(
    creator: &SigningKey,
    created_at: i64,
    nickname: &str,
) -> (ChessGameStateV1, ChessGameParametersV1) {
    let setup = GameSetup::mine(
        &creator.verifying_key(),
        Color::White,
        TimeControl::default(),
        created_at,
        nickname.to_string(),
    );
    let game_id = setup.derive_game_id(&creator.verifying_key());
    let params = ChessGameParametersV1 {
        creator: creator.verifying_key(),
        game_id,
    };
    let state = ChessGameStateV1 {
        setup: GameSetupV1(Some(setup.sign(creator, &game_id))),
        ..Default::default()
    };
    (state, params)
}

/// The opponent seat filled the legitimate way: the challenger offers and the
/// creator countersigns. Nothing else fills it — see [`crate::game::opponent`].
pub fn seated_slot(
    creator: &SigningKey,
    joiner: &SigningKey,
    params: &ChessGameParametersV1,
    at: i64,
    nickname: &str,
) -> OpponentSlotV1 {
    let join = SignedJoin::new(joiner, &params.game_id, at, nickname.to_string());
    OpponentSlotV1 {
        offers: Default::default(),
        seated: Some(SignedAcceptance::new(creator, &params.game_id, join)),
        declined: None,
    }
}

/// A game with both players seated.
pub fn started_game(
    creator: &SigningKey,
    opponent: &SigningKey,
    created_at: i64,
) -> (ChessGameStateV1, ChessGameParametersV1) {
    let (mut state, params) = open_game(creator, created_at, "creator");
    state.opponent = seated_slot(creator, opponent, &params, created_at + 1000, "opponent");
    (state, params)
}

/// Play `line` (UCI) into a started game, alternating signers from White.
pub fn play_line(
    state: &mut ChessGameStateV1,
    params: &ChessGameParametersV1,
    white: &SigningKey,
    black: &SigningKey,
    line: &[&str],
    start_at: i64,
) {
    for (i, uci) in line.iter().enumerate() {
        let ply = i as u32;
        let signer = if ply % 2 == 0 { white } else { black };
        let mv = Move::from_uci(uci).expect("valid uci");
        let am = AuthorizedMove::new(
            signer,
            &params.game_id,
            ply,
            mv,
            start_at + (ply as i64 + 1) * 1000,
        );
        state.moves.moves.insert(ply, am);
    }
}

/// A finished, co-signed game: fool's mate, Black wins by checkmate.
pub fn certified_game(
    white: &SigningKey,
    black: &SigningKey,
    created_at: i64,
    white_rating: i32,
    black_rating: i32,
) -> (GameCertificate, GameId) {
    let (mut state, params) = started_game(white, black, created_at);
    play_line(
        &mut state,
        &params,
        white,
        black,
        &["f2f3", "e7e5", "g2g4", "d8h4"],
        created_at + 1000,
    );
    state
        .verify(&state, &params)
        .expect("fixture game must be valid");

    let draft = CertificateDraft::from_game(
        &state,
        params.game_id,
        created_at + 60_000,
        white_rating,
        black_rating,
    )
    .expect("game is finished");
    let ws = draft.sign(white);
    let bs = draft.sign(black);
    let cert = draft.assemble(ws, bs);
    cert.verify().expect("fixture certificate must be valid");
    (cert, params.game_id)
}
