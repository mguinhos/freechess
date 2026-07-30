//! Tests for the game contract's state.
//!
//! Two things matter most here and both are tested directly:
//!
//! 1. **Authorization** — only the two players of *this* game can move, and
//!    only on their own turn.
//! 2. **Convergence** — merging in any order, any number of times, reaches the
//!    same state (the idempotent commutative monoid the platform requires).

use super::conclusion::{ConclusionKind, SignedConclusion};
use super::moves::AuthorizedMove;
use super::opponent::SignedJoin;
use super::setup::{leading_zero_bits, GameSetup, TimeControl, POW_DIFFICULTY_BITS};
use super::*;
use crate::chess::{Color, Move};
use ed25519_dalek::SigningKey;
use freenet_scaffold::ComposableState;
use rand::rngs::OsRng;

const T0: i64 = 1_700_000_000_000;

fn key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

/// A game created by `creator` playing `creator_plays`, with nobody joined yet.
fn open_game(
    creator: &SigningKey,
    creator_plays: Color,
) -> (ChessGameStateV1, ChessGameParametersV1) {
    open_game_at(creator, creator_plays, T0)
}

/// As [`open_game`], but with an explicit creation time — which is part of the
/// proof-of-work preimage, so two games by the same creator get distinct ids.
fn open_game_at(
    creator: &SigningKey,
    creator_plays: Color,
    created_at: i64,
) -> (ChessGameStateV1, ChessGameParametersV1) {
    let setup = GameSetup::mine(
        &creator.verifying_key(),
        creator_plays,
        TimeControl::default(),
        created_at,
        "creator".to_string(),
    );
    let game_id = setup.derive_game_id(&creator.verifying_key());
    let params = ChessGameParametersV1 {
        creator: creator.verifying_key(),
        game_id,
    };
    let signed = setup.sign(creator, &game_id);
    let state = ChessGameStateV1 {
        setup: setup::GameSetupV1(Some(signed)),
        ..Default::default()
    };
    (state, params)
}

/// A game with both seats filled: creator plays White, `opponent` plays Black.
fn started_game(
    creator: &SigningKey,
    opponent: &SigningKey,
) -> (ChessGameStateV1, ChessGameParametersV1) {
    started_game_at(creator, opponent, T0)
}

fn started_game_at(
    creator: &SigningKey,
    opponent: &SigningKey,
    created_at: i64,
) -> (ChessGameStateV1, ChessGameParametersV1) {
    let (mut state, params) = open_game_at(creator, Color::White, created_at);
    let join = SignedJoin::new(
        opponent,
        &params.game_id,
        created_at + 1000,
        "opponent".to_string(),
    );
    state.opponent = opponent::OpponentSlotV1(Some(join));
    (state, params)
}

/// Append a move signed by `signer`, without any validation.
fn push_move(
    state: &mut ChessGameStateV1,
    params: &ChessGameParametersV1,
    signer: &SigningKey,
    ply: u32,
    uci: &str,
    at: i64,
) {
    let mv = Move::from_uci(uci).expect("valid uci");
    let am = AuthorizedMove::new(signer, &params.game_id, ply, mv, at);
    state.moves.moves.insert(ply, am);
}

// ---------------------------------------------------------------- setup / PoW

#[test]
fn mined_setup_satisfies_the_difficulty_and_verifies() {
    let creator = key();
    let (state, params) = open_game(&creator, Color::White);
    assert!(leading_zero_bits(&params.game_id.0) >= POW_DIFFICULTY_BITS);
    assert!(state.verify(&state, &params).is_ok());
}

#[test]
fn setup_with_insufficient_proof_of_work_is_rejected() {
    let creator = key();
    // A nonce of 0 is overwhelmingly unlikely to satisfy 16 bits, and we assert
    // the premise rather than assuming it.
    let setup = GameSetup {
        creator_plays: Color::White,
        time_control: TimeControl::default(),
        created_at: T0,
        pow_nonce: 0,
        creator_nickname: "spammer".to_string(),
        challenged: None,
    };
    let game_id = setup.derive_game_id(&creator.verifying_key());
    assert!(
        leading_zero_bits(&game_id.0) < POW_DIFFICULTY_BITS,
        "test premise: nonce 0 must not accidentally satisfy the difficulty"
    );

    let params = ChessGameParametersV1 {
        creator: creator.verifying_key(),
        game_id,
    };
    let state = ChessGameStateV1 {
        setup: setup::GameSetupV1(Some(setup.sign(&creator, &game_id))),
        ..Default::default()
    };
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("proof-of-work"), "unexpected error: {err}");
}

#[test]
fn setup_signed_by_someone_other_than_the_creator_is_rejected() {
    let creator = key();
    let impostor = key();
    let (mut state, params) = open_game(&creator, Color::White);

    // Re-sign the same terms with a different key and swap the creator field.
    let mut signed = state.setup.0.clone().unwrap();
    signed.creator = impostor.verifying_key();
    state.setup = setup::GameSetupV1(Some(signed));

    assert!(state.verify(&state, &params).is_err());
}

#[test]
fn tampering_with_any_signed_setup_field_breaks_verification() {
    let creator = key();
    let (state, params) = open_game(&creator, Color::White);

    // Time control, color and nickname are all covered by the signature.
    let mut tampered = state.clone();
    let mut s = tampered.setup.0.clone().unwrap();
    s.setup.time_control.initial_secs = 60;
    tampered.setup = setup::GameSetupV1(Some(s));
    assert!(tampered.verify(&tampered, &params).is_err());

    let mut tampered = state.clone();
    let mut s = tampered.setup.0.clone().unwrap();
    s.setup.creator_plays = Color::Black;
    tampered.setup = setup::GameSetupV1(Some(s));
    assert!(tampered.verify(&tampered, &params).is_err());

    let mut tampered = state.clone();
    let mut s = tampered.setup.0.clone().unwrap();
    s.setup.creator_nickname = "someone else".to_string();
    tampered.setup = setup::GameSetupV1(Some(s));
    assert!(tampered.verify(&tampered, &params).is_err());
}

#[test]
fn absurd_time_controls_are_rejected() {
    let creator = key();
    let (mut state, params) = open_game(&creator, Color::White);
    let mut s = state.setup.0.clone().unwrap();
    s.setup.time_control.initial_secs = 1; // below the floor
                                           // Re-derive and re-sign so only the bounds check can fail it.
    let game_id = s.setup.derive_game_id(&creator.verifying_key());
    let params = ChessGameParametersV1 { game_id, ..params };
    state.setup = setup::GameSetupV1(Some(s.setup.clone().sign(&creator, &game_id)));
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("initial time"), "unexpected error: {err}");
}

// -------------------------------------------------------------------- joining

#[test]
fn the_creator_cannot_join_their_own_game() {
    let creator = key();
    let (mut state, params) = open_game(&creator, Color::White);
    let join = SignedJoin::new(&creator, &params.game_id, T0 + 1, "me again".to_string());
    state.opponent = opponent::OpponentSlotV1(Some(join));
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("cannot join their own game"), "got: {err}");
}

#[test]
fn a_join_race_resolves_identically_regardless_of_order() {
    let creator = key();
    let alice = key();
    let bob = key();
    let (base, params) = open_game(&creator, Color::White);

    let join_a = SignedJoin::new(&alice, &params.game_id, T0 + 500, "alice".to_string());
    let join_b = SignedJoin::new(&bob, &params.game_id, T0 + 700, "bob".to_string());

    // Peer 1 sees Alice then Bob; peer 2 sees Bob then Alice.
    let mut peer1 = base.clone();
    peer1
        .opponent
        .apply_delta(&base, &params, &Some(join_a.clone()))
        .unwrap();
    peer1
        .opponent
        .apply_delta(&base, &params, &Some(join_b.clone()))
        .unwrap();

    let mut peer2 = base.clone();
    peer2
        .opponent
        .apply_delta(&base, &params, &Some(join_b.clone()))
        .unwrap();
    peer2
        .opponent
        .apply_delta(&base, &params, &Some(join_a.clone()))
        .unwrap();

    assert_eq!(peer1.opponent, peer2.opponent, "race must converge");
    // Alice joined earlier, so she wins under the total order.
    assert_eq!(
        peer1.opponent.get().unwrap().player,
        alice.verifying_key(),
        "earliest join must win"
    );

    // And re-merging is a no-op.
    let before = peer1.opponent.clone();
    peer1
        .opponent
        .apply_delta(&base, &params, &Some(join_b))
        .unwrap();
    assert_eq!(before, peer1.opponent, "merge must be idempotent");
}

#[test]
fn a_third_party_cannot_displace_a_seated_opponent() {
    let creator = key();
    let opponent = key();
    let latecomer = key();
    let (mut state, params) = open_game(&creator, Color::White);

    let first = SignedJoin::new(&opponent, &params.game_id, T0 + 100, "first".to_string());
    state
        .opponent
        .apply_delta(&state.clone(), &params, &Some(first))
        .unwrap();

    // A later join, however well-formed, loses the total order.
    let late = SignedJoin::new(&latecomer, &params.game_id, T0 + 900, "late".to_string());
    let before = state.opponent.clone();
    state
        .opponent
        .apply_delta(&state.clone(), &params, &Some(late))
        .unwrap();
    assert_eq!(before, state.opponent);
    assert_eq!(
        state.opponent.get().unwrap().player,
        opponent.verifying_key()
    );
}

// ---------------------------------------------------- direct challenges

/// A game created as a direct challenge to `invited`.
fn challenge_game(
    creator: &SigningKey,
    invited: &SigningKey,
) -> (ChessGameStateV1, ChessGameParametersV1) {
    let setup = GameSetup::mine_challenge(
        &creator.verifying_key(),
        Color::White,
        TimeControl::default(),
        T0,
        "creator".to_string(),
        Some(invited.verifying_key()),
    );
    let game_id = setup.derive_game_id(&creator.verifying_key());
    let params = ChessGameParametersV1 {
        creator: creator.verifying_key(),
        game_id,
    };
    let state = ChessGameStateV1 {
        setup: setup::GameSetupV1(Some(setup.sign(creator, &game_id))),
        ..Default::default()
    };
    (state, params)
}

#[test]
fn only_the_invited_player_can_accept_a_direct_challenge() {
    let creator = key();
    let invited = key();
    let gatecrasher = key();
    let (base, params) = challenge_game(&creator, &invited);

    // Someone else grabbing the seat must be rejected.
    let mut state = base.clone();
    state.opponent = opponent::OpponentSlotV1(Some(SignedJoin::new(
        &gatecrasher,
        &params.game_id,
        T0 + 1000,
        "gatecrasher".to_string(),
    )));
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("direct challenge"), "unexpected error: {err}");

    // The invited player is admitted.
    let mut state = base;
    state.opponent = opponent::OpponentSlotV1(Some(SignedJoin::new(
        &invited,
        &params.game_id,
        T0 + 1000,
        "invited".to_string(),
    )));
    state
        .verify(&state, &params)
        .expect("the invited player may accept");
}

#[test]
fn a_gatecrasher_is_refused_on_the_merge_path_too() {
    let creator = key();
    let invited = key();
    let gatecrasher = key();
    let (base, params) = challenge_game(&creator, &invited);

    let mut state = base.clone();
    let err = state
        .opponent
        .apply_delta(
            &base,
            &params,
            &Some(SignedJoin::new(
                &gatecrasher,
                &params.game_id,
                T0 + 1000,
                "gatecrasher".to_string(),
            )),
        )
        .expect_err("must refuse");
    assert!(err.contains("direct challenge"), "unexpected error: {err}");
    assert!(state.opponent.get().is_none(), "the seat must stay empty");
}

#[test]
fn the_challenged_field_is_covered_by_the_creators_signature() {
    let creator = key();
    let invited = key();
    let attacker = key();
    let (mut state, params) = challenge_game(&creator, &invited);

    // Redirect the invitation to the attacker without re-signing.
    let mut signed = state.setup.0.clone().unwrap();
    signed.setup.challenged = Some(attacker.verifying_key());
    state.setup = setup::GameSetupV1(Some(signed));

    assert!(
        state.verify(&state, &params).is_err(),
        "tampering with the invitee must break the signature"
    );
}

#[test]
fn a_creator_cannot_challenge_themselves() {
    let creator = key();
    let (state, params) = challenge_game(&creator, &creator);
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("challenge themselves"), "got: {err}");
}

// -------------------------------------------------------------- move authority

#[test]
fn a_spectator_cannot_move() {
    let creator = key();
    let opponent = key();
    let spectator = key();
    let (mut state, params) = started_game(&creator, &opponent);

    // A perfectly well-formed, correctly signed move — by the wrong person.
    push_move(&mut state, &params, &spectator, 0, "e2e4", T0 + 2000);

    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(
        err.contains("does not own the side to move"),
        "unexpected error: {err}"
    );

    // And the prune path drops it rather than accepting it.
    state.prune(&params).unwrap();
    assert!(state.moves.is_empty(), "spectator move must not survive");
}

#[test]
fn a_player_cannot_move_on_the_opponents_turn() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);

    // Creator is White. Ply 1 belongs to Black, but White signs it.
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);
    push_move(&mut state, &params, &creator, 1, "e7e5", T0 + 3000);

    assert!(state.verify(&state, &params).is_err());
    state.prune(&params).unwrap();
    assert_eq!(state.moves.len(), 1, "only White's own move survives");
}

#[test]
fn no_move_is_accepted_before_an_opponent_joins() {
    let creator = key();
    let (mut state, params) = open_game(&creator, Color::White);
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);

    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("before an opponent joined"), "got: {err}");

    state.prune(&params).unwrap();
    assert!(state.moves.is_empty());
}

#[test]
fn a_move_signed_for_a_different_game_is_rejected() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    // A *second* game between the same two players. Created a moment later, so
    // the proof-of-work preimage differs and the two games get distinct ids —
    // which is the whole point: authority in one game must not carry into the
    // other.
    let (_, other_params) = started_game_at(&creator, &opponent, T0 + 60_000);
    assert_ne!(params.game_id, other_params.game_id);

    // Sign a move for the *other* game, then paste it into this one. This is
    // the concrete "one game's authority must not carry to another" check.
    let mv = Move::from_uci("e2e4").unwrap();
    let am = AuthorizedMove::new(&creator, &other_params.game_id, 0, mv, T0 + 2000);
    state.moves.moves.insert(0, am);

    assert!(state.verify(&state, &params).is_err());
    state.prune(&params).unwrap();
    assert!(state.moves.is_empty());
}

#[test]
fn an_illegal_move_cannot_enter_state() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    // Correctly signed by the right player, but the knight cannot reach e5.
    push_move(&mut state, &params, &creator, 0, "b1e5", T0 + 2000);

    assert!(state.verify(&state, &params).is_err());
    state.prune(&params).unwrap();
    assert!(state.moves.is_empty());
}

#[test]
fn a_legal_game_verifies_and_replays() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);

    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);
    push_move(&mut state, &params, &opponent, 1, "e7e5", T0 + 3000);
    push_move(&mut state, &params, &creator, 2, "g1f3", T0 + 4000);

    state.verify(&state, &params).expect("legal game verifies");
    assert_eq!(state.replay().san_list(), vec!["e4", "e5", "Nf3"]);
    assert_eq!(state.result(), GameResult::InProgress);
}

#[test]
fn a_gap_in_the_move_list_is_rejected() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);
    // Ply 1 missing.
    push_move(&mut state, &params, &creator, 2, "g1f3", T0 + 4000);

    assert!(state.verify(&state, &params).is_err());
    state.prune(&params).unwrap();
    assert_eq!(state.moves.len(), 1);
}

#[test]
fn moves_going_backwards_in_time_are_rejected() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 5000);
    push_move(&mut state, &params, &opponent, 1, "e7e5", T0 + 2000);

    assert!(state.verify(&state, &params).is_err());
}

// -------------------------------------------------------------- convergence

#[test]
fn concurrent_moves_converge_regardless_of_merge_order() {
    let creator = key();
    let opponent = key();
    let (base, params) = started_game(&creator, &opponent);

    let m0 = AuthorizedMove::new(
        &creator,
        &params.game_id,
        0,
        Move::from_uci("e2e4").unwrap(),
        T0 + 2000,
    );
    let m1 = AuthorizedMove::new(
        &opponent,
        &params.game_id,
        1,
        Move::from_uci("e7e5").unwrap(),
        T0 + 3000,
    );

    let mut peer1 = base.clone();
    peer1
        .moves
        .apply_delta(&base, &params, &Some(vec![m0.clone()]))
        .unwrap();
    peer1
        .moves
        .apply_delta(&base, &params, &Some(vec![m1.clone()]))
        .unwrap();
    peer1.prune(&params).unwrap();

    let mut peer2 = base.clone();
    peer2
        .moves
        .apply_delta(&base, &params, &Some(vec![m1.clone()]))
        .unwrap();
    peer2
        .moves
        .apply_delta(&base, &params, &Some(vec![m0.clone()]))
        .unwrap();
    peer2.prune(&params).unwrap();

    assert_eq!(peer1, peer2, "merge must be commutative");
    assert_eq!(peer1.moves.len(), 2);

    // Idempotence: applying everything again changes nothing.
    let before = peer1.clone();
    peer1
        .moves
        .apply_delta(&base, &params, &Some(vec![m0, m1]))
        .unwrap();
    peer1.prune(&params).unwrap();
    assert_eq!(before, peer1, "merge must be idempotent");
}

#[test]
fn a_double_signed_ply_resolves_to_the_same_move_on_every_peer() {
    let creator = key();
    let opponent = key();
    let (base, params) = started_game(&creator, &opponent);

    // The same player signs two different legal moves for ply 0 — equivocation.
    let a = AuthorizedMove::new(
        &creator,
        &params.game_id,
        0,
        Move::from_uci("e2e4").unwrap(),
        T0 + 2000,
    );
    let b = AuthorizedMove::new(
        &creator,
        &params.game_id,
        0,
        Move::from_uci("d2d4").unwrap(),
        T0 + 2000,
    );

    let mut peer1 = base.clone();
    peer1
        .moves
        .apply_delta(&base, &params, &Some(vec![a.clone(), b.clone()]))
        .unwrap();
    let mut peer2 = base.clone();
    peer2
        .moves
        .apply_delta(&base, &params, &Some(vec![b, a]))
        .unwrap();

    assert_eq!(peer1.moves, peer2.moves, "equivocation must still converge");
    assert_eq!(peer1.moves.len(), 1, "only one move can occupy a ply");
}

#[test]
fn prune_is_idempotent() {
    let creator = key();
    let opponent = key();
    let spectator = key();
    let (mut state, params) = started_game(&creator, &opponent);

    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);
    push_move(&mut state, &params, &opponent, 1, "e7e5", T0 + 3000);
    push_move(&mut state, &params, &spectator, 2, "g1f3", T0 + 4000);

    state.prune(&params).unwrap();
    let once = state.clone();
    state.prune(&params).unwrap();
    assert_eq!(once, state, "prune(prune(s)) must equal prune(s)");
    assert_eq!(state.moves.len(), 2);
}

#[test]
fn summary_and_delta_round_trip() {
    let creator = key();
    let opponent = key();
    let (base, params) = started_game(&creator, &opponent);

    let mut ahead = base.clone();
    push_move(&mut ahead, &params, &creator, 0, "e2e4", T0 + 2000);
    push_move(&mut ahead, &params, &opponent, 1, "e7e5", T0 + 3000);

    // A peer holding `base` summarizes; the peer holding `ahead` computes what
    // is missing; applying it must bring them level.
    let summary = base.summarize(&base, &params);
    let delta = ahead.delta(&ahead, &params, &summary).expect("has a delta");

    let mut catching_up = base.clone();
    catching_up
        .apply_delta(&base.clone(), &params, &Some(delta))
        .unwrap();

    assert_eq!(catching_up.moves, ahead.moves);
}

// -------------------------------------------------------------- conclusions

#[test]
fn resignation_hands_the_win_to_the_opponent() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);

    // Creator plays White and resigns.
    let c = SignedConclusion::resign(&creator, &params.game_id, 1, T0 + 5000);
    state.conclusion = conclusion::ConclusionV1(Some(c));

    state.verify(&state, &params).expect("resignation is valid");
    assert_eq!(
        state.result(),
        GameResult::BlackWins(WinReason::Resignation)
    );
}

#[test]
fn an_outsider_cannot_resign_someone_elses_game() {
    let creator = key();
    let opponent = key();
    let outsider = key();
    let (mut state, params) = started_game(&creator, &opponent);

    let c = SignedConclusion::resign(&outsider, &params.game_id, 0, T0 + 5000);
    state.conclusion = conclusion::ConclusionV1(Some(c));

    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("not a player in this game"), "got: {err}");
}

#[test]
fn a_draw_needs_both_signatures() {
    let creator = key();
    let opponent = key();
    let outsider = key();
    let (mut state, params) = started_game(&creator, &opponent);

    // Genuine agreement verifies.
    let good = SignedConclusion::draw_agreement(&creator, &opponent, &params.game_id, 0, T0 + 5000);
    state.conclusion = conclusion::ConclusionV1(Some(good));
    state.verify(&state, &params).expect("agreed draw is valid");
    assert_eq!(state.result(), GameResult::Draw(DrawReason::Agreement));

    // A "draw" one player signed twice is not an agreement.
    let forged =
        SignedConclusion::draw_agreement(&creator, &creator, &params.game_id, 0, T0 + 5000);
    state.conclusion = conclusion::ConclusionV1(Some(forged));
    assert!(state.verify(&state, &params).is_err());

    // Nor is one countersigned by a bystander.
    let forged =
        SignedConclusion::draw_agreement(&creator, &outsider, &params.game_id, 0, T0 + 5000);
    state.conclusion = conclusion::ConclusionV1(Some(forged));
    assert!(state.verify(&state, &params).is_err());
}

#[test]
fn a_timeout_cannot_be_claimed_early_but_can_be_claimed_late() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    // Creator (White) moves; Black's clock now runs from T0+2000.
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);

    let budget_ms = TimeControl::default().initial_secs as i64 * 1000;

    // One second in: nowhere near flag fall.
    let early = SignedConclusion::claim_timeout(&creator, &params.game_id, 1, T0 + 3000);
    state.conclusion = conclusion::ConclusionV1(Some(early));
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("too early"), "got: {err}");

    // Well past the budget: the claim stands.
    let late = SignedConclusion::claim_timeout(
        &creator,
        &params.game_id,
        1,
        T0 + 2000 + budget_ms + 60_000,
    );
    state.conclusion = conclusion::ConclusionV1(Some(late));
    state.verify(&state, &params).expect("timeout is provable");
    assert_eq!(state.result(), GameResult::WhiteWins(WinReason::Timeout));
}

#[test]
fn a_player_cannot_claim_a_timeout_against_a_clock_that_is_not_running() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    // No moves yet: it is White's (the creator's) turn, so White cannot claim
    // that Black flagged.
    let budget_ms = TimeControl::default().initial_secs as i64 * 1000;
    let claim =
        SignedConclusion::claim_timeout(&creator, &params.game_id, 0, T0 + budget_ms + 60_000);
    state.conclusion = conclusion::ConclusionV1(Some(claim));
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("too early"), "got: {err}");
}

#[test]
fn concurrent_conclusions_converge() {
    let creator = key();
    let opponent = key();
    let (base, params) = started_game(&creator, &opponent);

    let a = SignedConclusion::resign(&creator, &params.game_id, 0, T0 + 5000);
    let b = SignedConclusion::resign(&opponent, &params.game_id, 0, T0 + 6000);

    let mut peer1 = base.clone();
    peer1
        .conclusion
        .apply_delta(&base, &params, &Some(a.clone()))
        .unwrap();
    peer1
        .conclusion
        .apply_delta(&base, &params, &Some(b.clone()))
        .unwrap();

    let mut peer2 = base.clone();
    peer2
        .conclusion
        .apply_delta(&base, &params, &Some(b))
        .unwrap();
    peer2
        .conclusion
        .apply_delta(&base, &params, &Some(a))
        .unwrap();

    assert_eq!(peer1.conclusion, peer2.conclusion);
    // Earliest claim wins the total order.
    assert_eq!(peer1.conclusion.get().unwrap().at, T0 + 5000);
}

// ------------------------------------------------------------------- clocks

#[test]
fn clocks_are_derived_from_signed_timestamps() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    let start = T0 + 1000; // the join time

    // White thinks 3s, Black thinks 5s.
    push_move(&mut state, &params, &creator, 0, "e2e4", start + 3_000);
    push_move(&mut state, &params, &opponent, 1, "e7e5", start + 8_000);
    state.verify(&state, &params).unwrap();

    assert_eq!(state.time_used_by(Color::White), 3_000);
    assert_eq!(state.time_used_by(Color::Black), 5_000);

    let budget = TimeControl::default().initial_secs as i64 * 1000;
    // At the instant of Black's move it is White's turn again, so White has
    // nothing pending yet.
    assert_eq!(
        state.time_remaining(Color::White, start + 8_000),
        budget - 3_000
    );
    // Ten more seconds pass with White on the clock.
    assert_eq!(
        state.time_remaining(Color::White, start + 18_000),
        budget - 13_000
    );
    // Black's clock is idle in the meantime.
    assert_eq!(
        state.time_remaining(Color::Black, start + 18_000),
        budget - 5_000
    );
}

#[test]
fn increment_is_credited_per_move_made() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = open_game(&creator, Color::White);
    // Rebuild the setup with an increment.
    let setup = GameSetup::mine(
        &creator.verifying_key(),
        Color::White,
        TimeControl {
            initial_secs: 600,
            increment_secs: 5,
        },
        T0,
        "creator".to_string(),
    );
    let game_id = setup.derive_game_id(&creator.verifying_key());
    let params = ChessGameParametersV1 { game_id, ..params };
    state.setup = setup::GameSetupV1(Some(setup.sign(&creator, &game_id)));
    state.opponent = opponent::OpponentSlotV1(Some(SignedJoin::new(
        &opponent,
        &game_id,
        T0 + 1000,
        "opponent".to_string(),
    )));

    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 4000);
    state.verify(&state, &params).unwrap();

    // White spent 3s but earned 5s of increment for the move made.
    let budget = 600 * 1000 + 5 * 1000;
    assert_eq!(
        state.time_remaining(Color::White, T0 + 4000),
        budget - 3_000
    );
}

#[test]
fn a_challenger_cannot_backdate_a_join_to_steal_clock_time() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = open_game(&creator, Color::White);
    // Join claiming to have happened long before the game was created.
    let join = SignedJoin::new(
        &opponent,
        &params.game_id,
        T0 - 900_000,
        "sneaky".to_string(),
    );
    state.opponent = opponent::OpponentSlotV1(Some(join));

    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);
    state.verify(&state, &params).unwrap();

    // Clocks start at creation, not the back-dated join, so White is charged
    // 2 seconds — not fifteen minutes.
    assert_eq!(state.time_used_by(Color::White), 2_000);
}

// ------------------------------------------------------------------- results

#[test]
fn checkmate_on_the_board_ends_the_game_without_any_signed_conclusion() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);

    // Fool's mate: White is mated on Black's second move.
    let line = [
        (0u32, "f2f3", &creator),
        (1, "e7e5", &opponent),
        (2, "g2g4", &creator),
        (3, "d8h4", &opponent),
    ];
    for (ply, uci, signer) in line {
        push_move(
            &mut state,
            &params,
            signer,
            ply,
            uci,
            T0 + 2000 + ply as i64 * 1000,
        );
    }

    state.verify(&state, &params).unwrap();
    assert_eq!(state.result(), GameResult::BlackWins(WinReason::Checkmate));
    assert!(state.conclusion.get().is_none());
}

#[test]
fn an_open_game_reports_itself_as_awaiting_an_opponent() {
    let creator = key();
    let (state, _params) = open_game(&creator, Color::White);
    assert!(state.is_open());
    assert_eq!(state.result(), GameResult::AwaitingOpponent);
}

#[test]
fn colors_follow_the_creators_choice() {
    let creator = key();
    let opponent = key();

    let (mut state, params) = open_game(&creator, Color::Black);
    state.opponent = opponent::OpponentSlotV1(Some(SignedJoin::new(
        &opponent,
        &params.game_id,
        T0 + 1000,
        "opponent".to_string(),
    )));

    // Creator chose Black, so the challenger is White and moves first.
    assert_eq!(state.color_of(&creator.verifying_key()), Some(Color::Black));
    assert_eq!(
        state.color_of(&opponent.verifying_key()),
        Some(Color::White)
    );
    assert_eq!(state.key_for_ply(0), Some(opponent.verifying_key()));
}
