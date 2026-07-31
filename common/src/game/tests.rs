//! Tests for the game contract's state.
//!
//! Two things matter most here and both are tested directly:
//!
//! 1. **Authorization** — only the two players of *this* game can move, and
//!    only on their own turn.
//! 2. **Convergence** — merging in any order, any number of times, reaches the
//!    same state (the idempotent commutative monoid the platform requires).

use super::clocks::{self, ClockAttestation};
use super::conclusion::{self, SignedConclusion};
use super::moves::AuthorizedMove;
use super::opponent::{OpponentDelta, SignedAcceptance, SignedDecline, SignedJoin};
use super::setup::{
    leading_zero_bits, GameSetup, TimeControl, MAX_INCREMENT_SECS, MAX_INITIAL_SECS,
    MIN_INITIAL_SECS, POW_DIFFICULTY_BITS,
};
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
    state.opponent = seat(creator, opponent, &params, created_at + 1000, "opponent");
    (state, params)
}

/// A seat filled the legitimate way: `joiner` offers, the creator countersigns.
fn seat(
    creator: &SigningKey,
    joiner: &SigningKey,
    params: &ChessGameParametersV1,
    at: i64,
    nickname: &str,
) -> opponent::OpponentSlotV1 {
    let join = SignedJoin::new(joiner, &params.game_id, at, nickname.to_string());
    opponent::OpponentSlotV1 {
        offers: Default::default(),
        seated: Some(SignedAcceptance::new(creator, &params.game_id, join)),
        declined: None,
    }
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
    // Mine the absurd terms rather than patching them in afterwards: the game id
    // covers every term, so editing one invalidates the proof-of-work and the
    // bounds check would never be reached.
    let setup = GameSetup::mine(
        &creator.verifying_key(),
        Color::White,
        TimeControl {
            initial_secs: 1, // below the floor
            increment_secs: 0,
        },
        T0,
        "creator".to_string(),
    );
    let game_id = setup.derive_game_id(&creator.verifying_key());
    let params = ChessGameParametersV1 {
        creator: creator.verifying_key(),
        game_id,
    };
    let state = ChessGameStateV1 {
        setup: setup::GameSetupV1(Some(setup.sign(&creator, &game_id))),
        ..Default::default()
    };
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("initial time"), "unexpected error: {err}");
}

// -------------------------------------------------------------------- joining

#[test]
fn the_creator_cannot_join_their_own_game() {
    let creator = key();
    let (mut state, params) = open_game(&creator, Color::White);
    state.opponent = seat(&creator, &creator, &params, T0 + 1, "me again");
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("cannot join their own game"), "got: {err}");
}

#[test]
fn competing_offers_converge_and_seat_nobody_on_their_own() {
    let creator = key();
    let alice = key();
    let bob = key();
    let (base, params) = open_game(&creator, Color::White);

    let offer_a = OpponentDelta::offer(SignedJoin::new(
        &alice,
        &params.game_id,
        T0 + 500,
        "alice".to_string(),
    ));
    let offer_b = OpponentDelta::offer(SignedJoin::new(
        &bob,
        &params.game_id,
        T0 + 700,
        "bob".to_string(),
    ));

    // Peer 1 sees Alice then Bob; peer 2 sees Bob then Alice.
    let mut peer1 = base.clone();
    peer1
        .opponent
        .apply_delta(&base, &params, &Some(offer_a.clone()))
        .unwrap();
    peer1
        .opponent
        .apply_delta(&base, &params, &Some(offer_b.clone()))
        .unwrap();

    let mut peer2 = base.clone();
    peer2
        .opponent
        .apply_delta(&base, &params, &Some(offer_b.clone()))
        .unwrap();
    peer2
        .opponent
        .apply_delta(&base, &params, &Some(offer_a))
        .unwrap();

    assert_eq!(peer1.opponent, peer2.opponent, "offers must converge");
    assert!(
        peer1.opponent.get().is_none(),
        "an offer alone must not seat anyone — only the creator can"
    );
    assert_eq!(peer1.opponent.pending_offers().len(), 2);

    // Re-merging changes nothing.
    let before = peer1.opponent.clone();
    peer1
        .opponent
        .apply_delta(&base, &params, &Some(offer_b))
        .unwrap();
    assert_eq!(before, peer1.opponent, "merge must be idempotent");
}

/// The creator picks, and the pick is what starts the game.
#[test]
fn the_creators_countersignature_is_what_fills_the_seat() {
    let creator = key();
    let alice = key();
    let bob = key();
    let (base, params) = open_game(&creator, Color::White);

    let alice_join = SignedJoin::new(&alice, &params.game_id, T0 + 500, "alice".to_string());
    let bob_join = SignedJoin::new(&bob, &params.game_id, T0 + 700, "bob".to_string());

    let mut state = base.clone();
    for join in [alice_join, bob_join.clone()] {
        state
            .opponent
            .apply_delta(&base, &params, &Some(OpponentDelta::offer(join)))
            .unwrap();
    }

    // The creator seats Bob, the later offer — proving the seat follows the
    // countersignature and not the clock.
    let accepted = SignedAcceptance::new(&creator, &params.game_id, bob_join);
    state
        .opponent
        .apply_delta(&base, &params, &Some(OpponentDelta::seat(accepted)))
        .unwrap();
    state.prune(&params).unwrap();

    assert_eq!(state.opponent.get().unwrap().player, bob.verifying_key());
    assert!(
        state.opponent.pending_offers().is_empty(),
        "offers are dropped once the seat is filled"
    );
    state.verify(&state, &params).expect("state must validate");
}

/// Nobody but the creator can seat a player — the acceptance is checked against
/// the creator key in the contract parameters, which is part of the address.
#[test]
fn an_acceptance_signed_by_anyone_else_is_refused() {
    let creator = key();
    let challenger = key();
    let impostor = key();
    let (base, params) = open_game(&creator, Color::White);

    let join = SignedJoin::new(&challenger, &params.game_id, T0 + 500, "c".to_string());
    let forged = SignedAcceptance::new(&impostor, &params.game_id, join);

    let mut state = base.clone();
    let err = state
        .opponent
        .apply_delta(&base, &params, &Some(OpponentDelta::seat(forged.clone())))
        .expect_err("must refuse");
    assert!(err.contains("seat acceptance"), "unexpected error: {err}");
    assert!(state.opponent.get().is_none(), "the seat must stay empty");

    // And the same on the full-state path, which skips the merge.
    let mut state = base;
    state.opponent.seated = Some(forged);
    assert!(state.verify(&state, &params).is_err());
}

/// An offer with no proof-of-work is refused, so filling the offer list costs
/// the same work as creating a game.
#[test]
fn an_offer_without_proof_of_work_is_refused() {
    let creator = key();
    let challenger = key();
    let (base, params) = open_game(&creator, Color::White);

    let mut join = SignedJoin::new(&challenger, &params.game_id, T0 + 500, "c".to_string());
    join.pow_nonce = join.pow_nonce.wrapping_add(1);

    let err = state_offer_error(&base, &params, join);
    assert!(err.contains("proof-of-work"), "unexpected error: {err}");
}

fn state_offer_error(
    base: &ChessGameStateV1,
    params: &ChessGameParametersV1,
    join: SignedJoin,
) -> String {
    let mut state = base.clone();
    state
        .opponent
        .apply_delta(base, params, &Some(OpponentDelta::offer(join)))
        .expect_err("must refuse")
}

#[test]
fn a_third_party_cannot_displace_a_seated_opponent() {
    let creator = key();
    let opponent = key();
    let latecomer = key();
    let (mut state, params) = open_game(&creator, Color::White);
    let base = state.clone();

    state.opponent = seat(&creator, &opponent, &params, T0 + 100, "first");

    // A later offer, however well-formed, cannot take a filled seat: it is only
    // an offer, and prune drops it outright.
    let late = SignedJoin::new(&latecomer, &params.game_id, T0 + 900, "late".to_string());
    state
        .opponent
        .apply_delta(&base, &params, &Some(OpponentDelta::offer(late)))
        .unwrap();
    state.prune(&params).unwrap();
    assert_eq!(
        state.opponent.get().unwrap().player,
        opponent.verifying_key()
    );
    assert!(state.opponent.pending_offers().is_empty());
}

/// Backdating a join must not evict a seated opponent, nor erase the moves they
/// have already played. The race order is `(joined_at, key)` and `joined_at` is
/// the joiner's own unverifiable claim, so without a floor an attacker simply
/// signs `joined_at = 1` and wins every race — retroactively, in the middle of a
/// game in progress.
#[test]
fn a_backdated_join_cannot_hijack_a_game_in_progress() {
    let creator = key();
    let opponent = key();
    let attacker = key();
    let (mut state, params) = open_game(&creator, Color::White);

    state.opponent = seat(&creator, &opponent, &params, T0 + 100, "honest");

    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 200);
    push_move(&mut state, &params, &opponent, 1, "e7e5", T0 + 300);
    state.prune(&params).unwrap();
    assert_eq!(state.moves.len(), 2, "both moves stand before the attack");

    // The attacker claims to have joined before the honest player did.
    let backdated = SignedJoin::new(&attacker, &params.game_id, 1, "attacker".to_string());
    let outcome = state.opponent.apply_delta(
        &state.clone(),
        &params,
        &Some(OpponentDelta::offer(backdated.clone())),
    );
    state.prune(&params).unwrap();

    assert!(
        outcome.is_err() || state.opponent.get().unwrap().player == opponent.verifying_key(),
        "a backdated join took the seat of a player already in the game"
    );
    assert_eq!(
        state.moves.len(),
        2,
        "the seated player's moves were erased by a backdated join"
    );

    // Nor does self-signing an acceptance help: it is checked against the
    // creator's key, which lives in the contract parameters.
    let self_seated = SignedAcceptance::new(&attacker, &params.game_id, backdated);
    assert!(state
        .opponent
        .apply_delta(
            &state.clone(),
            &params,
            &Some(OpponentDelta::seat(self_seated))
        )
        .is_err());
}

// ---------------------------------------------------------- declining

#[test]
fn the_invited_player_can_decline_and_the_seat_closes() {
    let creator = key();
    let invited = key();
    let (mut state, params) = challenge_game(&creator, &invited);
    let base = state.clone();

    // Someone offers first — a declined challenge must clear that too, since
    // nobody is ever going to be seated.
    state
        .opponent
        .apply_delta(
            &base,
            &params,
            &Some(OpponentDelta::offer(SignedJoin::new(
                &invited,
                &params.game_id,
                T0 + 500,
                "invited".to_string(),
            ))),
        )
        .unwrap();

    let decline = SignedDecline::new(&invited, &params.game_id, T0 + 1000);
    state
        .opponent
        .apply_delta(&base, &params, &Some(OpponentDelta::decline(decline)))
        .unwrap();
    state.prune(&params).unwrap();

    assert!(state.opponent.is_declined());
    assert!(state.opponent.get().is_none(), "nobody may be seated");
    assert!(state.opponent.pending_offers().is_empty());
    state.verify(&state, &params).expect("state must validate");
}

#[test]
fn only_the_challenged_player_may_decline() {
    let creator = key();
    let invited = key();
    let stranger = key();
    let (state, params) = challenge_game(&creator, &invited);

    for (who, label) in [(&stranger, "a stranger"), (&creator, "the creator")] {
        let forged = SignedDecline::new(who, &params.game_id, T0 + 1000);
        let err = forged
            .verify(&params, Some(invited.verifying_key()))
            .unwrap_err();
        assert!(err.contains("only the challenged player"), "{label}: {err}");

        let mut merged = state.clone();
        merged
            .opponent
            .apply_delta(&state, &params, &Some(OpponentDelta::decline(forged)))
            .expect_err("the merge path must refuse it too");
    }
}

/// An open game has nobody in particular to refuse it, so there is nothing a
/// decline could mean.
#[test]
fn an_open_game_cannot_be_declined() {
    let creator = key();
    let passer_by = key();
    let (_state, params) = open_game(&creator, Color::White);

    let decline = SignedDecline::new(&passer_by, &params.game_id, T0 + 1000);
    let err = decline.verify(&params, None).unwrap_err();
    assert!(err.contains("only a direct challenge"), "got: {err}");
}

/// If the creator managed to seat someone, the game is on — a decline arriving
/// afterwards must not leave the state implying otherwise.
#[test]
fn a_filled_seat_outranks_a_late_decline() {
    let creator = key();
    let invited = key();
    let (mut state, params) = challenge_game(&creator, &invited);
    let base = state.clone();

    state.opponent = seat(&creator, &invited, &params, T0 + 500, "invited");
    state
        .opponent
        .apply_delta(
            &base,
            &params,
            &Some(OpponentDelta::decline(SignedDecline::new(
                &invited,
                &params.game_id,
                T0 + 900,
            ))),
        )
        .unwrap();
    state.prune(&params).unwrap();

    assert!(!state.opponent.is_declined());
    assert_eq!(
        state.opponent.get().unwrap().player,
        invited.verifying_key(),
        "a seated game must stay seated"
    );
}

#[test]
fn declining_converges_regardless_of_order() {
    let creator = key();
    let invited = key();
    let (base, params) = challenge_game(&creator, &invited);

    let offer = OpponentDelta::offer(SignedJoin::new(
        &invited,
        &params.game_id,
        T0 + 500,
        "invited".to_string(),
    ));
    let decline = OpponentDelta::decline(SignedDecline::new(&invited, &params.game_id, T0 + 1000));

    let mut peer1 = base.clone();
    let mut peer2 = base.clone();
    for (peer, order) in [
        (&mut peer1, [offer.clone(), decline.clone()]),
        (&mut peer2, [decline, offer]),
    ] {
        for d in order {
            peer.opponent.apply_delta(&base, &params, &Some(d)).unwrap();
        }
        peer.prune(&params).unwrap();
    }
    assert_eq!(peer1.opponent, peer2.opponent, "must converge");
    assert!(peer1.opponent.is_declined());
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
    state.opponent = seat(&creator, &gatecrasher, &params, T0 + 1000, "gatecrasher");
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("direct challenge"), "unexpected error: {err}");

    // The invited player is admitted.
    let mut state = base;
    state.opponent = seat(&creator, &invited, &params, T0 + 1000, "invited");
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
            &Some(OpponentDelta::offer(SignedJoin::new(
                &gatecrasher,
                &params.game_id,
                T0 + 1000,
                "gatecrasher".to_string(),
            ))),
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
    state.conclusion = conclusion::ConclusionV1::single(c);

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
    state.conclusion = conclusion::ConclusionV1::single(c);

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
    state.conclusion = conclusion::ConclusionV1::single(good);
    state.verify(&state, &params).expect("agreed draw is valid");
    assert_eq!(state.result(), GameResult::Draw(DrawReason::Agreement));

    // A "draw" one player signed twice is not an agreement.
    let forged =
        SignedConclusion::draw_agreement(&creator, &creator, &params.game_id, 0, T0 + 5000);
    state.conclusion = conclusion::ConclusionV1::single(forged);
    assert!(state.verify(&state, &params).is_err());

    // Nor is one countersigned by a bystander.
    let forged =
        SignedConclusion::draw_agreement(&creator, &outsider, &params.game_id, 0, T0 + 5000);
    state.conclusion = conclusion::ConclusionV1::single(forged);
    assert!(state.verify(&state, &params).is_err());
}

#[test]
fn a_timeout_holds_once_the_loser_attested_their_way_to_flag_fall() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    // Creator (White) moves; Black's clock now runs from T0+2000.
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);

    let budget_ms = TimeControl::default().initial_secs as i64 * 1000;
    let flag_fall = T0 + 2000 + budget_ms;

    // Nowhere near flag fall, and nothing corroborates the claim.
    let early = SignedConclusion::claim_timeout(&creator, &params.game_id, 1, T0 + 3000);
    state.conclusion = conclusion::ConclusionV1::single(early);
    state.verify(&state, &params).expect("authentic claim");
    assert!(!state.result().is_over(), "an early claim must be inert");

    // Black keeps signing right up to the moment their clock hits zero, which
    // is exactly the evidence that makes the flag fall provable.
    state.clocks.attestations.insert(
        PlayerId::from(&opponent.verifying_key()),
        ClockAttestation::new(&opponent, &params.game_id, flag_fall),
    );
    let claim = SignedConclusion::claim_timeout(&creator, &params.game_id, 1, flag_fall);
    state.conclusion = conclusion::ConclusionV1::single(claim);
    state.verify(&state, &params).expect("timeout is provable");
    assert_eq!(state.result(), GameResult::WhiteWins(WinReason::Timeout));
}

#[test]
fn a_player_cannot_claim_a_timeout_against_a_clock_that_is_not_running() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    // No moves yet: it is White's (the creator's) turn, so White cannot claim
    // that Black flagged, however much evidence Black has signed.
    let budget_ms = TimeControl::default().initial_secs as i64 * 1000;
    state.clocks.attestations.insert(
        PlayerId::from(&opponent.verifying_key()),
        ClockAttestation::new(&opponent, &params.game_id, T0 + budget_ms + 60_000),
    );
    let claim =
        SignedConclusion::claim_timeout(&creator, &params.game_id, 0, T0 + budget_ms + 60_000);
    state.conclusion = conclusion::ConclusionV1::single(claim);
    state.verify(&state, &params).expect("authentic claim");
    assert!(
        !state.result().is_over(),
        "Black's clock is not running, so the claim cannot hold"
    );
    assert_eq!(state.timeout_provable_at(Color::Black, 0), None);
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
        .apply_delta(&base, &params, &Some(vec![a.clone()]))
        .unwrap();
    peer1
        .conclusion
        .apply_delta(&base, &params, &Some(vec![b.clone()]))
        .unwrap();

    let mut peer2 = base.clone();
    peer2
        .conclusion
        .apply_delta(&base, &params, &Some(vec![b]))
        .unwrap();
    peer2
        .conclusion
        .apply_delta(&base, &params, &Some(vec![a]))
        .unwrap();

    assert_eq!(peer1.conclusion, peer2.conclusion);
    // Earliest claim wins the total order.
    assert_eq!(peer1.conclusion.effective(&peer1).unwrap().at, T0 + 5000);
}

#[test]
fn two_peers_holding_different_conclusions_exchange_them() {
    // The summary used to be a bare `bool`, so both peers reported "I have a
    // conclusion", neither shipped a delta, and the total order that settles
    // them was unreachable — a permanent divergence on the game's result.
    let creator = key();
    let opponent = key();
    let (base, params) = started_game(&creator, &opponent);

    let mut peer1 = base.clone();
    peer1.conclusion = conclusion::ConclusionV1::single(SignedConclusion::resign(
        &creator,
        &params.game_id,
        0,
        T0 + 6000,
    ));
    let mut peer2 = base.clone();
    peer2.conclusion = conclusion::ConclusionV1::single(SignedConclusion::resign(
        &opponent,
        &params.game_id,
        0,
        T0 + 5000,
    ));

    let summary = peer1.summarize(&peer1, &params);
    let delta = peer2
        .delta(&peer2, &params, &summary)
        .expect("peer2 holds a claim peer1 does not");
    peer1.apply_delta(&base, &params, &Some(delta)).unwrap();

    // Both claims are now present and the earlier one decides the game.
    assert_eq!(peer1.conclusion.claims.len(), 2);
    assert_eq!(peer1.conclusion.effective(&peer1).unwrap().at, T0 + 5000);
    assert_eq!(
        peer1.result(),
        GameResult::WhiteWins(WinReason::Resignation)
    );
}

#[test]
fn an_inert_timeout_claim_cannot_bury_a_real_resignation() {
    // With one slot and an earliest-wins order, a bogus timeout dated at the
    // epoch would occupy it forever and hang the game.
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);

    let bogus = SignedConclusion::claim_timeout(&creator, &params.game_id, 1, 1);
    let real = SignedConclusion::resign(&opponent, &params.game_id, 1, T0 + 5000);
    state
        .conclusion
        .apply_delta(&state.clone(), &params, &Some(vec![bogus, real]))
        .unwrap();

    state.verify(&state, &params).expect("both are authentic");
    assert_eq!(
        state.result(),
        GameResult::WhiteWins(WinReason::Resignation)
    );
}

#[test]
fn a_present_players_next_attestation_strips_a_pre_signed_absence_claim() {
    // An absence claim can be pre-signed, so it can briefly hold against a
    // player who is really there. Their next tick must void it permanently.
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);

    // Black last attested at T0+3000, so absence matures 45s later.
    let last_tick = T0 + 3000;
    let insert = |state: &mut ChessGameStateV1, at: i64| {
        state.clocks.attestations.insert(
            PlayerId::from(&opponent.verifying_key()),
            ClockAttestation::new(&opponent, &params.game_id, at),
        );
    };
    insert(&mut state, last_tick);

    let deadline = last_tick + clocks::absence_forfeit_ms(&TimeControl::default());
    assert_eq!(state.timeout_provable_at(Color::Black, 1), Some(deadline));

    let claim = SignedConclusion::claim_timeout(&creator, &params.game_id, 1, deadline);
    state.conclusion = conclusion::ConclusionV1::single(claim);
    assert_eq!(state.result(), GameResult::WhiteWins(WinReason::Timeout));

    // Black was at the board all along and ticks again. The deadline moves past
    // the claim, which is now inert — and stays inert, because attested time
    // only ever advances.
    insert(&mut state, last_tick + clocks::CLOCK_TICK_MS);
    state.verify(&state, &params).expect("still authentic");
    assert!(
        !state.result().is_over(),
        "a live opponent's own signature must defeat the claim"
    );
}

#[test]
fn a_player_who_stops_attesting_forfeits_without_burning_their_whole_clock() {
    // The common real timeout: the opponent closes the tab. Waiting out ten
    // minutes of their clock is not required, because their signatures stopped.
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);

    let vanished_at = T0 + 3000;
    state.clocks.attestations.insert(
        PlayerId::from(&opponent.verifying_key()),
        ClockAttestation::new(&opponent, &params.game_id, vanished_at),
    );

    let deadline = vanished_at + clocks::absence_forfeit_ms(&TimeControl::default());
    let claim = SignedConclusion::claim_timeout(&creator, &params.game_id, 1, deadline);
    state.conclusion = conclusion::ConclusionV1::single(claim);
    state.verify(&state, &params).unwrap();
    assert_eq!(state.result(), GameResult::WhiteWins(WinReason::Timeout));

    // Well short of the ten-minute budget.
    assert!(deadline < T0 + 2000 + 600_000);
}

#[test]
fn a_stale_timeout_claim_is_inert() {
    // `at` is pinned to the instant the claim matured, so a claim cannot carry
    // an arbitrary finish time into a certificate or the archive's ordering.
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);
    state.clocks.attestations.insert(
        PlayerId::from(&opponent.verifying_key()),
        ClockAttestation::new(&opponent, &params.game_id, T0 + 3000),
    );

    let deadline = state.timeout_provable_at(Color::Black, 1).unwrap();
    let stale = SignedConclusion::claim_timeout(
        &creator,
        &params.game_id,
        1,
        deadline + conclusion::TIMEOUT_CLAIM_WINDOW_MS + 1,
    );
    state.conclusion = conclusion::ConclusionV1::single(stale);
    assert!(!state.result().is_over());
}

#[test]
fn an_attestation_from_a_bystander_is_pruned_and_rejected() {
    let creator = key();
    let opponent = key();
    let outsider = key();
    let (mut state, params) = started_game(&creator, &opponent);

    state.clocks.attestations.insert(
        PlayerId::from(&outsider.verifying_key()),
        ClockAttestation::new(&outsider, &params.game_id, T0 + 3000),
    );
    let err = state.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("not a player"), "got: {err}");

    state.prune(&params).unwrap();
    assert!(state.clocks.attestations.is_empty());
}

#[test]
fn attestations_converge_on_the_latest_regardless_of_arrival_order() {
    let creator = key();
    let opponent = key();
    let (base, params) = started_game(&creator, &opponent);

    let early = ClockAttestation::new(&opponent, &params.game_id, T0 + 3000);
    let late = ClockAttestation::new(&opponent, &params.game_id, T0 + 9000);

    let mut peer1 = base.clone();
    peer1
        .clocks
        .apply_delta(&base, &params, &Some(vec![early.clone(), late.clone()]))
        .unwrap();
    let mut peer2 = base.clone();
    peer2
        .clocks
        .apply_delta(&base, &params, &Some(vec![late, early]))
        .unwrap();

    assert_eq!(peer1.clocks, peer2.clocks);
    assert_eq!(
        peer1.clocks.attested_at(&opponent.verifying_key()),
        Some(T0 + 9000)
    );
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
    state.opponent = seat(&creator, &opponent, &params, T0 + 1000, "opponent");

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
    // Join claiming to have happened long before the game was created — and
    // countersigned, so the seat is legitimate; only the claimed time is a lie.
    state.opponent = seat(&creator, &opponent, &params, T0 - 900_000, "sneaky");

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
    assert!(state.conclusion.is_empty());
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
    state.opponent = seat(&creator, &opponent, &params, T0 + 1000, "opponent");

    // Creator chose Black, so the challenger is White and moves first.
    assert_eq!(state.color_of(&creator.verifying_key()), Some(Color::Black));
    assert_eq!(
        state.color_of(&opponent.verifying_key()),
        Some(Color::White)
    );
    assert_eq!(state.key_for_ply(0), Some(opponent.verifying_key()));
}

#[test]
fn a_future_dated_timeout_claim_cannot_steal_a_win() {
    // A contract has no clock, so nothing in state contradicts a claim stamped
    // days from now. Without corroboration from the loser's own signatures, any
    // player can end any game at any ply by claiming their opponent flagged.
    let creator = key();
    let opponent = key();
    let (mut state, params) = started_game(&creator, &opponent);
    push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);
    push_move(&mut state, &params, &opponent, 1, "e7e5", T0 + 3000);

    // White is to move and has spent one second of ten minutes.
    assert_eq!(state.time_remaining(Color::White, T0 + 3000), 599_000);

    // Black claims White flagged, dating the claim eleven days out.
    let forged = SignedConclusion::claim_timeout(&opponent, &params.game_id, 2, T0 + 1_000_000_000);
    state.conclusion = conclusion::ConclusionV1::single(forged);

    assert!(
        state.verify(&state, &params).is_err() || !state.result().is_over(),
        "a claim with no corroborating evidence must not end the game; got {:?}",
        state.result()
    );
}

#[test]
fn the_game_id_pins_every_term_of_the_setup() {
    // The id is the proof-of-work digest and the contract parameter, and
    // `apply_delta` is write-once on the strength of "a verified setup is
    // uniquely determined by the parameters". That only holds if the id covers
    // every signed term. When it covered just (creator, created_at, nonce), one
    // creator could sign two valid setups disagreeing on who plays White and
    // what the clock is, and peers kept whichever landed first — a permanent
    // divergence on the game's terms.
    let creator = key();
    let ck = creator.verifying_key();

    let base = GameSetup::mine(
        &ck,
        Color::White,
        TimeControl::default(),
        T0,
        "creator".to_string(),
    );
    let game_id = base.derive_game_id(&ck);

    // Same creator, same created_at, same nonce, different terms.
    let swapped = GameSetup {
        creator_plays: Color::Black,
        ..base.clone()
    };
    let faster = GameSetup {
        time_control: TimeControl {
            initial_secs: 60,
            increment_secs: 0,
        },
        ..base.clone()
    };
    let renamed = GameSetup {
        creator_nickname: "someone else".to_string(),
        ..base.clone()
    };
    let invited = GameSetup {
        challenged: Some(key().verifying_key()),
        ..base.clone()
    };

    for (what, other) in [
        ("colors", swapped),
        ("time control", faster),
        ("nickname", renamed),
        ("challenge", invited),
    ] {
        assert_ne!(
            other.derive_game_id(&ck),
            game_id,
            "{what} must change the game id"
        );

        // And so the alternative cannot verify under the original parameters.
        let params = ChessGameParametersV1 {
            creator: ck,
            game_id,
        };
        let state = ChessGameStateV1 {
            setup: setup::GameSetupV1(Some(other.clone().sign(&creator, &game_id))),
            ..Default::default()
        };
        assert!(
            state.verify(&state, &params).is_err(),
            "a setup differing in {what} must not verify under the original game id"
        );
    }
}

#[test]
fn two_offers_from_one_key_still_exchange() {
    // The summary must distinguish the offers a peer holds, or two peers each
    // holding a different offer from the *same* challenger report identical
    // summaries, neither ships, and the creator sees a different set on each
    // peer — so which one it can countersign depends on where it is looking.
    let creator = key();
    let challenger = key();
    let (base, params) = open_game(&creator, Color::White);

    let early = SignedJoin::new(&challenger, &params.game_id, T0 + 1_000, "c".to_string());
    let late = SignedJoin::new(&challenger, &params.game_id, T0 + 9_000, "c".to_string());

    let mut peer1 = base.clone();
    peer1
        .opponent
        .apply_delta(&base, &params, &Some(OpponentDelta::offer(late)))
        .unwrap();
    let mut peer2 = base.clone();
    peer2
        .opponent
        .apply_delta(&base, &params, &Some(OpponentDelta::offer(early)))
        .unwrap();

    for _ in 0..2 {
        let s1 = peer1.opponent.summarize(&peer1, &params);
        if let Some(d) = peer2.opponent.delta(&peer2, &params, &s1) {
            peer1
                .opponent
                .apply_delta(&peer1.clone(), &params, &Some(d))
                .unwrap();
        }
        let s2 = peer2.opponent.summarize(&peer2, &params);
        if let Some(d) = peer1.opponent.delta(&peer1, &params, &s2) {
            peer2
                .opponent
                .apply_delta(&peer2.clone(), &params, &Some(d))
                .unwrap();
        }
    }

    assert_eq!(peer1.opponent, peer2.opponent);
}

/// The seat converges even if the creator countersigns two different offers —
/// misbehaviour by the creator must not split the network.
#[test]
fn a_doubly_countersigned_seat_still_converges() {
    let creator = key();
    let alice = key();
    let bob = key();
    let (base, params) = open_game(&creator, Color::White);

    let seat_a = SignedAcceptance::new(
        &creator,
        &params.game_id,
        SignedJoin::new(&alice, &params.game_id, T0 + 500, "alice".to_string()),
    );
    let seat_b = SignedAcceptance::new(
        &creator,
        &params.game_id,
        SignedJoin::new(&bob, &params.game_id, T0 + 700, "bob".to_string()),
    );

    let mut peer1 = base.clone();
    let mut peer2 = base.clone();
    for (peer, order) in [
        (&mut peer1, [seat_a.clone(), seat_b.clone()]),
        (&mut peer2, [seat_b.clone(), seat_a.clone()]),
    ] {
        for acceptance in order {
            peer.opponent
                .apply_delta(&base, &params, &Some(OpponentDelta::seat(acceptance)))
                .unwrap();
        }
    }

    assert_eq!(
        peer1.opponent, peer2.opponent,
        "a double acceptance must still converge"
    );
    assert!(peer1.opponent.get().is_some());
}

#[test]
fn the_merged_state_always_survives_validation() {
    // The node calls `validate_state` on whatever `update_state` returns and
    // discards the update if it fails, so `verify` must accept everything the
    // merge path can produce. This is why a timeout claim's corroboration lives
    // in `is_effective` and not in `verify`: corroboration depends on the
    // loser's attestations, which keep arriving, so a claim that was provable
    // when stored can stop being provable later. Checking that in `verify` would
    // mean a later, entirely honest attestation delta could no longer be
    // persisted by any peer holding the earlier claim.
    let creator = key();
    let opponent = key();
    let (state, params) = started_game(&creator, &opponent);

    let mut merged = state.clone();
    let apply = |merged: &mut ChessGameStateV1, delta: ChessGameStateV1Delta| {
        let snapshot = merged.clone();
        merged
            .apply_delta(&snapshot, &params, &Some(delta))
            .expect("delta applies");
        // `post_apply_delta = "prune"` runs here in the generated code.
        merged.verify(&*merged, &params).expect(
            "every state the merge path can produce must pass validation, \
             or the node discards the update and nothing persists",
        );
    };

    // A move, then an attestation from each player.
    apply(
        &mut merged,
        ChessGameStateV1Delta {
            moves: Some(vec![AuthorizedMove::new(
                &creator,
                &params.game_id,
                0,
                Move::from_uci("e2e4").unwrap(),
                T0 + 2_000,
            )]),
            ..Default::default()
        },
    );
    for k in [&creator, &opponent] {
        apply(
            &mut merged,
            ChessGameStateV1Delta {
                clocks: Some(vec![ClockAttestation::new(k, &params.game_id, T0 + 3_000)]),
                ..Default::default()
            },
        );
    }

    // A timeout claim that no evidence supports. It is stored and inert.
    apply(
        &mut merged,
        ChessGameStateV1Delta {
            conclusion: Some(vec![SignedConclusion::claim_timeout(
                &creator,
                &params.game_id,
                1,
                T0 + 4_000,
            )]),
            ..Default::default()
        },
    );
    assert!(!merged.result().is_over(), "the claim must be inert");

    // Now the loser keeps attesting, which is what strips such a claim. The
    // state must still validate — this is the case that would break if
    // corroboration were enforced in `verify`.
    for tick in 1..=6 {
        apply(
            &mut merged,
            ChessGameStateV1Delta {
                clocks: Some(vec![ClockAttestation::new(
                    &opponent,
                    &params.game_id,
                    T0 + 3_000 + tick * clocks::CLOCK_TICK_MS,
                )]),
                ..Default::default()
            },
        );
    }

    // And a resignation still decides the game despite the inert claim sitting
    // in the set alongside it.
    apply(
        &mut merged,
        ChessGameStateV1Delta {
            conclusion: Some(vec![SignedConclusion::resign(
                &opponent,
                &params.game_id,
                1,
                T0 + 9_000,
            )]),
            ..Default::default()
        },
    );
    assert_eq!(
        merged.result(),
        GameResult::WhiteWins(WinReason::Resignation)
    );
}

#[test]
fn two_moves_signed_for_one_ply_still_exchange() {
    // `absorb` settles a double-signed ply by signature bytes, but the summary
    // was the ply list alone. Two peers each holding a different move for the
    // same ply reported identical summaries, neither shipped, and the boards
    // diverged permanently.
    let creator = key();
    let opponent = key();
    let (base, params) = started_game(&creator, &opponent);

    let one = AuthorizedMove::new(
        &creator,
        &params.game_id,
        0,
        Move::from_uci("e2e4").unwrap(),
        T0 + 2_000,
    );
    let two = AuthorizedMove::new(
        &creator,
        &params.game_id,
        0,
        Move::from_uci("d2d4").unwrap(),
        T0 + 2_000,
    );
    let winner = if one.signature.to_bytes() < two.signature.to_bytes() {
        one.clone()
    } else {
        two.clone()
    };

    let mut peer1 = base.clone();
    peer1.moves.moves.insert(0, one);
    let mut peer2 = base.clone();
    peer2.moves.moves.insert(0, two);

    for _ in 0..2 {
        let s1 = peer1.moves.summarize(&peer1, &params);
        if let Some(d) = peer2.moves.delta(&peer2, &params, &s1) {
            peer1
                .moves
                .apply_delta(&peer1.clone(), &params, &Some(d))
                .unwrap();
        }
        let s2 = peer2.moves.summarize(&peer2, &params);
        if let Some(d) = peer1.moves.delta(&peer1, &params, &s2) {
            peer2
                .moves
                .apply_delta(&peer2.clone(), &params, &Some(d))
                .unwrap();
        }
    }

    assert_eq!(peer1.moves, peer2.moves, "both peers must reach one board");
    assert_eq!(peer1.moves.moves.get(&0), Some(&winner));
}

// --------------------------------------------------- convergence harness

/// Drives `freenet_scaffold`'s convergence framework over the whole game state.
///
/// The platform's requirement on a contract is algebraic: the merge must be
/// commutative, associative and idempotent, and the whitepaper is explicit that
/// it "cannot statically verify the algebraic properties; a contract that
/// violates them will fail to converge". The hand-written tests above each pin
/// one specific way that could break. This pins the general property, over
/// permuted orderings of a mixed operation stream, across every field at once —
/// which is what actually catches an interaction nobody thought to write a test
/// for.
#[derive(Clone)]
struct GameHarness {
    white: SigningKey,
    black: SigningKey,
    state: ChessGameStateV1,
    params: ChessGameParametersV1,
    /// A fixed legal line, so ply *n* always carries the same move whatever
    /// order the operations arrive in.
    line: Vec<&'static str>,
    /// Whether to generate move operations. See
    /// [`the_game_state_merge_is_commutative`] for why the strict commutativity
    /// run leaves them out.
    include_moves: bool,
}

#[derive(Clone, Debug)]
enum Op {
    /// Play the move at this ply from the fixed line.
    PlayPly(usize),
    /// Attest a clock reading. `who` is 0 for White, 1 for Black.
    Attest { who: usize, tick: i64 },
    /// Resign, which is always decisive and so always changes the result.
    Resign { who: usize, at: i64 },
}

impl GameHarness {
    fn new() -> GameHarness {
        let white = key();
        let black = key();
        let (state, params) = started_game(&white, &black);
        GameHarness {
            white,
            black,
            state,
            params,
            line: vec![
                "e2e4", "e7e5", "g1f3", "b8c6", "f1c4", "f8c5", "d2d3", "d7d6",
            ],
            include_moves: true,
        }
    }

    fn without_moves() -> GameHarness {
        GameHarness {
            include_moves: false,
            ..GameHarness::new()
        }
    }

    fn signer(&self, who: usize) -> &SigningKey {
        if who == 0 {
            &self.white
        } else {
            &self.black
        }
    }
}

impl freenet_scaffold::convergence::ConvergenceTestHarness for GameHarness {
    type State = ChessGameStateV1;
    type Delta = ChessGameStateV1Delta;
    type Parameters = ChessGameParametersV1;
    type Operation = Op;

    fn initial_state(&self) -> (Self::State, Self::Parameters) {
        (self.state.clone(), self.params.clone())
    }

    fn generate_operation<R: freenet_scaffold::convergence::Rng>(&mut self, rng: &mut R) -> Op {
        match rng.gen_range(0..10) {
            0..=4 if self.include_moves => Op::PlayPly(rng.gen_range(0..self.line.len())),
            0..=4 => Op::Attest {
                who: rng.gen_range(0..2),
                tick: rng.gen_range(1..20) as i64,
            },
            5..=8 => Op::Attest {
                who: rng.gen_range(0..2),
                tick: rng.gen_range(1..20) as i64,
            },
            _ => Op::Resign {
                who: rng.gen_range(0..2),
                at: T0 + rng.gen_range(1..20) as i64 * 1000,
            },
        }
    }

    fn operation_to_delta(&mut self, _state: &Self::State, op: &Op) -> Self::Delta {
        match op {
            Op::PlayPly(ply) => {
                let signer = self.signer(ply % 2);
                let mv = Move::from_uci(self.line[*ply]).expect("fixture line is valid uci");
                ChessGameStateV1Delta {
                    moves: Some(vec![AuthorizedMove::new(
                        signer,
                        &self.params.game_id,
                        *ply as u32,
                        mv,
                        T0 + 2_000 + *ply as i64 * 1_000,
                    )]),
                    ..Default::default()
                }
            }
            Op::Attest { who, tick } => ChessGameStateV1Delta {
                clocks: Some(vec![ClockAttestation::new(
                    self.signer(*who),
                    &self.params.game_id,
                    T0 + tick * 1_000,
                )]),
                ..Default::default()
            },
            Op::Resign { who, at } => ChessGameStateV1Delta {
                conclusion: Some(vec![SignedConclusion::resign(
                    self.signer(*who),
                    &self.params.game_id,
                    0,
                    *at,
                )]),
                ..Default::default()
            },
        }
    }
}

#[test]
fn the_game_state_merge_is_commutative() {
    // Moves are excluded here, and the exclusion is a real finding rather than a
    // convenience: `MovesV1::prune` *clears* the list when a move arrives before
    // its predecessor, so applying plies out of order destroys them and the raw
    // delta merge is not commutative. See
    // `a_move_arriving_before_its_predecessor_is_dropped_but_heals` for the
    // behaviour and why it is survivable.
    let result = freenet_scaffold::convergence::test_operation_commutativity(
        GameHarness::without_moves(),
        12,
        25,
        0xC0FFEE,
    );
    assert!(result.passed, "{result:?}");
}

#[test]
fn the_game_state_merge_is_idempotent() {
    let result = freenet_scaffold::convergence::test_idempotency(GameHarness::new(), 20, 0xC0FFEE);
    assert!(result.passed, "{result:?}");
}

#[test]
fn a_move_arriving_before_its_predecessor_is_dropped_but_heals() {
    // Pins the known limitation the convergence harness surfaced.
    //
    // `prune` keeps the longest legal prefix from ply 0 and discards the rest,
    // so a move that arrives before the one it follows is destroyed outright.
    // The platform explicitly "tolerates arbitrary loss, reordering, and
    // duplication", so this ordering is not hypothetical.
    //
    // It is survivable because the summary carries the plies actually held: the
    // peer that dropped ply 1 no longer lists it, so the next anti-entropy round
    // sends it again, and this time it lands on top of ply 0 and sticks. The
    // cost is a wasted round, not a lost move — but note the merge is therefore
    // convergent only *through* the sync protocol, not per-delta.
    let creator = key();
    let opponent = key();
    let (base, params) = started_game(&creator, &opponent);

    let ply0 = AuthorizedMove::new(
        &creator,
        &params.game_id,
        0,
        Move::from_uci("e2e4").unwrap(),
        T0 + 2_000,
    );
    let ply1 = AuthorizedMove::new(
        &opponent,
        &params.game_id,
        1,
        Move::from_uci("e7e5").unwrap(),
        T0 + 3_000,
    );

    // Out of order: ply 1 first.
    let mut late = base.clone();
    for mv in [ply1.clone(), ply0.clone()] {
        let snapshot = late.clone();
        late.apply_delta(
            &snapshot,
            &params,
            &Some(ChessGameStateV1Delta {
                moves: Some(vec![mv]),
                ..Default::default()
            }),
        )
        .unwrap();
    }
    assert_eq!(late.moves.len(), 1, "ply 1 was dropped, as prune specifies");

    // A peer that saw them in order holds both, and anti-entropy closes the gap.
    let mut ordered = base.clone();
    let snapshot = ordered.clone();
    ordered
        .apply_delta(
            &snapshot,
            &params,
            &Some(ChessGameStateV1Delta {
                moves: Some(vec![ply0, ply1]),
                ..Default::default()
            }),
        )
        .unwrap();
    assert_eq!(ordered.moves.len(), 2);

    let summary = late.summarize(&late, &params);
    let delta = ordered
        .delta(&ordered, &params, &summary)
        .expect("the missing ply must be offered again");
    let snapshot = late.clone();
    late.apply_delta(&snapshot, &params, &Some(delta)).unwrap();

    assert_eq!(
        late.moves, ordered.moves,
        "the gap closes on the next round"
    );
}

#[test]
fn two_peers_seeing_different_operations_converge() {
    // This one goes through `ComposableState::merge`, so it exercises
    // summarize -> delta -> apply_delta rather than apply_delta alone: it is the
    // property that a summary too coarse for its merge order would break.
    for overlap in [0.0, 0.3, 0.7] {
        let result = freenet_scaffold::convergence::test_merge_convergence(
            GameHarness::new(),
            14,
            overlap,
            0xBEEF,
        );
        assert!(result.passed, "overlap {overlap}: {result:?}");
    }
}

// -------------------------------------------------------- certification

use super::certification::{CertificationV1, SignedCertificateProposal};
use crate::certificate::CertificateDraft;

/// A finished game: creator plays White and resigns, so Black wins.
fn finished_game(
    creator: &SigningKey,
    opponent: &SigningKey,
) -> (ChessGameStateV1, ChessGameParametersV1) {
    let (mut state, params) = started_game(creator, opponent);
    push_move(&mut state, &params, creator, 0, "e2e4", T0 + 2000);
    let c = SignedConclusion::resign(creator, &params.game_id, 1, T0 + 5000);
    state.conclusion = conclusion::ConclusionV1::single(c);
    state.prune(&params).unwrap();
    (state, params)
}

fn draft_for(
    state: &ChessGameStateV1,
    params: &ChessGameParametersV1,
    white_rating: i32,
    black_rating: i32,
    finished_at: i64,
) -> CertificateDraft {
    CertificateDraft::from_game(
        state,
        params.game_id,
        finished_at,
        white_rating,
        black_rating,
    )
    .expect("the game is over")
}

#[test]
fn one_signature_is_never_enough_to_certify() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = finished_game(&creator, &opponent);

    let draft = draft_for(&state, &params, 1200, 1200, T0 + 6000);
    let mine = SignedCertificateProposal::new(&creator, draft);
    state
        .certification
        .apply_delta(&state.clone(), &params, &Some(vec![mine]))
        .unwrap();

    assert!(
        state.certification.certificate(&state).is_none(),
        "a single player signed themselves a rated result"
    );
    state.verify(&state, &params).expect("state is valid");
}

#[test]
fn two_signatures_over_the_same_record_produce_a_certificate() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = finished_game(&creator, &opponent);

    let draft = draft_for(&state, &params, 1200, 1300, T0 + 6000);
    for signer in [&creator, &opponent] {
        let p = SignedCertificateProposal::new(signer, draft.clone());
        state
            .certification
            .apply_delta(&state.clone(), &params, &Some(vec![p]))
            .unwrap();
    }

    let cert = state
        .certification
        .certificate(&state)
        .expect("both halves are present");
    cert.verify()
        .expect("the assembled certificate must verify");
    assert_eq!(cert.game_id, params.game_id);
    assert_eq!(cert.white_rating_before, 1200);
    assert_eq!(cert.black_rating_before, 1300);
}

/// The reason the exchange exists: two clients cannot derive the same bytes,
/// so signing independently yields nothing until one adopts the other's draft.
#[test]
fn differing_drafts_certify_nothing_until_one_side_adopts_the_other() {
    let creator = key();
    let opponent = key();
    let (mut state, params) = finished_game(&creator, &opponent);

    // Same game, different wall clocks — exactly what happens in practice.
    let mine =
        SignedCertificateProposal::new(&creator, draft_for(&state, &params, 1200, 1200, T0 + 6000));
    let theirs = SignedCertificateProposal::new(
        &opponent,
        draft_for(&state, &params, 1200, 1200, T0 + 6001),
    );
    for p in [mine, theirs] {
        state
            .certification
            .apply_delta(&state.clone(), &params, &Some(vec![p]))
            .unwrap();
    }
    assert!(
        state.certification.certificate(&state).is_none(),
        "drafts that differ must not assemble"
    );

    // Both peers pick the same winner from the same set, and each re-signs
    // those exact bytes. Whichever player was already on it re-signs the same
    // record, which merges to a no-op — so neither client needs to work out
    // whose draft won, only that it is signing the winner.
    let winner = state.certification.winning_draft().unwrap().clone();
    for signer in [&creator, &opponent] {
        let adopted = SignedCertificateProposal::new(signer, winner.clone());
        state
            .certification
            .apply_delta(&state.clone(), &params, &Some(vec![adopted]))
            .unwrap();
    }

    let cert = state
        .certification
        .certificate(&state)
        .expect("adopting the winning draft completes it");
    cert.verify().unwrap();
    assert_eq!(cert.finished_at, winner.finished_at);
}

#[test]
fn a_stranger_cannot_certify_a_game_they_did_not_play() {
    let creator = key();
    let opponent = key();
    let outsider = key();
    let (state, params) = finished_game(&creator, &opponent);

    let forged = SignedCertificateProposal::new(
        &outsider,
        draft_for(&state, &params, 1200, 1200, T0 + 6000),
    );
    let err = forged
        .verify(&state, &params)
        .expect_err("an outsider must be refused");
    assert!(err.contains("only the two players"), "got: {err}");

    // And the merge path skips it rather than storing it.
    let mut merged = state.clone();
    merged
        .certification
        .apply_delta(&state, &params, &Some(vec![forged]))
        .unwrap();
    assert!(merged.certification.is_empty());
}

#[test]
fn a_draft_that_misreports_the_game_is_refused() {
    let creator = key();
    let opponent = key();
    let (state, params) = finished_game(&creator, &opponent);

    // Claim the other result. This is the lie a certificate exists to stop.
    let mut lying = draft_for(&state, &params, 1200, 1200, T0 + 6000);
    lying.result = GameResult::WhiteWins(WinReason::Resignation);
    let p = SignedCertificateProposal::new(&creator, lying);
    let err = p.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("different result"), "got: {err}");

    // Same for an invented move list, which would forge the replay.
    let mut padded = draft_for(&state, &params, 1200, 1200, T0 + 6000);
    padded.moves.push(Move::from_uci("e7e5").unwrap());
    let p = SignedCertificateProposal::new(&creator, padded);
    let err = p.verify(&state, &params).expect_err("must reject");
    assert!(err.contains("different move list"), "got: {err}");
}

#[test]
fn a_game_still_in_progress_cannot_be_certified() {
    let creator = key();
    let opponent = key();
    let (mut finished, params) = finished_game(&creator, &opponent);
    let draft = draft_for(&finished, &params, 1200, 1200, T0 + 6000);
    for signer in [&creator, &opponent] {
        let p = SignedCertificateProposal::new(signer, draft.clone());
        finished
            .certification
            .apply_delta(&finished.clone(), &params, &Some(vec![p]))
            .unwrap();
    }
    assert!(finished.certification.certificate(&finished).is_some());

    // Carry those proposals onto a game that is still running: prune drops
    // them, because a certificate for an undecided game means nothing.
    let (mut running, params2) = started_game(&creator, &opponent);
    running.certification = finished.certification.clone();
    running.prune(&params2).unwrap();
    assert!(running.certification.is_empty());
}

#[test]
fn certification_converges_regardless_of_order() {
    let creator = key();
    let opponent = key();
    let (base, params) = finished_game(&creator, &opponent);

    let draft = draft_for(&base, &params, 1200, 1200, T0 + 6000);
    let a = SignedCertificateProposal::new(&creator, draft.clone());
    let b = SignedCertificateProposal::new(&opponent, draft);

    let mut peer1 = base.clone();
    let mut peer2 = base.clone();
    for (peer, order) in [(&mut peer1, [a.clone(), b.clone()]), (&mut peer2, [b, a])] {
        for p in order {
            peer.certification
                .apply_delta(&base, &params, &Some(vec![p]))
                .unwrap();
        }
    }
    assert_eq!(peer1.certification, peer2.certification, "must converge");
    assert!(peer1.certification.certificate(&peer1).is_some());

    // And re-merging changes nothing.
    let before = peer1.certification.clone();
    let existing: Vec<_> = before.proposals.values().cloned().collect();
    peer1
        .certification
        .apply_delta(&base, &params, &Some(existing))
        .unwrap();
    assert_eq!(before, peer1.certification, "merge must be idempotent");
}

// -------------------------------------------------- absence, proportionally

/// The window before an absent player forfeits scales with the game, instead of
/// being a flat 45 seconds for every format.
///
/// The unfairness that motivated this is worth restating: away from the board in
/// real chess your clock runs and you lose when it reaches zero — you do not
/// lose *for being away*. A fixed window short-circuits that, and a player with
/// twenty minutes in hand could lose a won game because a backgrounded tab had
/// its timers throttled for a minute.
#[test]
fn the_absence_window_scales_with_the_time_control() {
    let bullet = TimeControl {
        initial_secs: 60,
        increment_secs: 0,
    };
    let blitz = TimeControl {
        initial_secs: 300,
        increment_secs: 0,
    };
    let rapid = TimeControl {
        initial_secs: 1800,
        increment_secs: 0,
    };

    let (b, z, r) = (
        clocks::absence_forfeit_ms(&bullet),
        clocks::absence_forfeit_ms(&blitz),
        clocks::absence_forfeit_ms(&rapid),
    );

    assert!(b <= z && z < r, "longer games give more rope: {b} {z} {r}");
    // Short formats land on the floor together, and that is the floor doing its
    // job rather than the scaling failing: a tenth of five minutes is exactly
    // the minimum, so bullet and blitz share it.
    assert_eq!(
        b,
        clocks::MIN_ABSENCE_FORFEIT_MS,
        "bullet sits on the floor"
    );
    assert_eq!(
        z,
        clocks::MIN_ABSENCE_FORFEIT_MS,
        "so does a five-minute game"
    );
    assert_eq!(r, 180_000, "a 30-minute game gives three minutes");

    // The increment counts towards the estimate, so a game that will genuinely
    // run long is treated as one.
    let with_increment = TimeControl {
        initial_secs: 300,
        increment_secs: 10,
    };
    assert!(
        clocks::absence_forfeit_ms(&with_increment) > z,
        "increment lengthens the game, so it lengthens the window"
    );
}

#[test]
fn the_absence_window_is_bounded_at_both_ends() {
    // Nothing may claim a game faster than the floor, however short the format.
    let instant = TimeControl {
        initial_secs: MIN_INITIAL_SECS,
        increment_secs: 0,
    };
    assert_eq!(
        clocks::absence_forfeit_ms(&instant),
        clocks::MIN_ABSENCE_FORFEIT_MS
    );

    // And a correspondence game does not become unreclaimable for hours.
    let correspondence = TimeControl {
        initial_secs: MAX_INITIAL_SECS,
        increment_secs: MAX_INCREMENT_SECS,
    };
    assert_eq!(
        clocks::absence_forfeit_ms(&correspondence),
        clocks::MAX_ABSENCE_FORFEIT_MS
    );
}

/// The scaling has to reach the rule itself, not just the helper.
#[test]
fn a_longer_game_postpones_the_absence_deadline() {
    let creator = key();
    let opponent = key();

    let deadline_for = |initial_secs: u32| -> i64 {
        let setup = GameSetup::mine(
            &creator.verifying_key(),
            Color::White,
            TimeControl {
                initial_secs,
                increment_secs: 0,
            },
            T0,
            "creator".to_string(),
        );
        let game_id = setup.derive_game_id(&creator.verifying_key());
        let params = ChessGameParametersV1 {
            creator: creator.verifying_key(),
            game_id,
        };
        let mut state = ChessGameStateV1 {
            setup: setup::GameSetupV1(Some(setup.sign(&creator, &game_id))),
            ..Default::default()
        };
        state.opponent = seat(&creator, &opponent, &params, T0 + 1000, "opponent");
        push_move(&mut state, &params, &creator, 0, "e2e4", T0 + 2000);
        state.prune(&params).unwrap();

        // Black is to move and stops attesting well before their flag falls.
        let vanished_at = T0 + 5000;
        state
            .clocks
            .apply_delta(
                &state.clone(),
                &params,
                &Some(vec![ClockAttestation::new(
                    &opponent,
                    &params.game_id,
                    vanished_at,
                )]),
            )
            .unwrap();
        state
            .timeout_provable_at(Color::Black, 1)
            .expect("absence is provable eventually")
    };

    let short = deadline_for(60);
    let long = deadline_for(1800);
    assert!(
        long > short,
        "the same absence should be forgiven longer in a longer game: {short} vs {long}"
    );
    assert_eq!(long - short, 180_000 - clocks::MIN_ABSENCE_FORFEIT_MS);
}
