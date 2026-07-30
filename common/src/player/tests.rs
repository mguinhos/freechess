//! Player profile tests: nicknames, history, and the derived rating.

use super::*;
use crate::testutil::{certified_game, key, T0};
use freenet_scaffold::ComposableState;

fn params_for(k: &ed25519_dalek::SigningKey) -> PlayerParametersV1 {
    PlayerParametersV1 {
        player: k.verifying_key(),
    }
}

// ---------------------------------------------------------------- nicknames

#[test]
fn a_player_can_choose_a_nickname() {
    let me = key();
    let params = params_for(&me);
    let mut state = PlayerStateV1::default();
    state.profile = ProfileV1(Some(SignedProfile::new(&me, "magnus".to_string(), T0)));

    state.verify(&state, &params).expect("profile must verify");
    assert_eq!(state.nickname(params.player_id()), "magnus");
}

#[test]
fn a_player_without_a_nickname_falls_back_to_a_short_id() {
    let me = key();
    let state = PlayerStateV1::default();
    let name = state.nickname(params_for(&me).player_id());
    assert!(name.starts_with("player-"), "got {name}");
}

#[test]
fn nobody_else_can_set_your_nickname() {
    let me = key();
    let impostor = key();
    let mut state = PlayerStateV1::default();
    // Signed by someone else, filed under my key.
    state.profile = ProfileV1(Some(SignedProfile::new(
        &impostor,
        "not me".to_string(),
        T0,
    )));
    assert!(state.verify(&state, &params_for(&me)).is_err());
}

#[test]
fn invalid_nicknames_are_rejected() {
    for bad in [
        "a",                                                   // too short
        " leading",                                            // leading whitespace
        "trailing ",                                           // trailing whitespace
        "with\nnewline",                                       // control character
        &"x".repeat(crate::game::setup::MAX_NICKNAME_LEN + 1), // too long
    ] {
        assert!(validate_nickname(bad).is_err(), "should reject {bad:?}");
    }
    for good in ["ab", "magnus", "Hikaru_99", "жуков", "chess♟"] {
        validate_nickname(good).unwrap_or_else(|e| panic!("should accept {good:?}: {e}"));
    }
}

#[test]
fn the_latest_nickname_wins_in_any_order() {
    let me = key();
    let params = params_for(&me);
    let old = SignedProfile::new(&me, "old".to_string(), T0);
    let new = SignedProfile::new(&me, "new".to_string(), T0 + 5000);
    let empty = PlayerStateV1::default();

    let mut peer1 = PlayerStateV1::default();
    peer1
        .profile
        .apply_delta(&empty, &params, &Some(old.clone()))
        .unwrap();
    peer1
        .profile
        .apply_delta(&empty, &params, &Some(new.clone()))
        .unwrap();

    let mut peer2 = PlayerStateV1::default();
    peer2
        .profile
        .apply_delta(&empty, &params, &Some(new))
        .unwrap();
    peer2
        .profile
        .apply_delta(&empty, &params, &Some(old))
        .unwrap();

    assert_eq!(peer1.profile, peer2.profile, "must converge");
    assert_eq!(peer1.nickname(params.player_id()), "new");
}

// ------------------------------------------------------------------ history

#[test]
fn a_certified_game_enters_the_players_history_and_moves_their_rating() {
    let winner = key();
    let loser = key();
    // Fool's mate: Black (the second key) wins.
    let (cert, game_id) = certified_game(&loser, &winner, T0, 1200, 1200);

    let params = params_for(&winner);
    let mut state = PlayerStateV1::default();
    state.history.games.insert(game_id, cert);
    state.verify(&state, &params).expect("history must verify");

    let id = params.player_id();
    assert_eq!(state.games_played(), 1);
    assert!(
        state.rating(id) > crate::elo::STARTING_ELO,
        "a win must raise the rating"
    );
    assert_eq!(state.history.record(id), (1, 0, 0));
}

#[test]
fn a_profile_cannot_claim_a_game_it_did_not_play() {
    let stranger = key();
    let a = key();
    let b = key();
    let (cert, game_id) = certified_game(&a, &b, T0, 1200, 1200);

    let mut state = PlayerStateV1::default();
    state.history.games.insert(game_id, cert);
    let err = state
        .verify(&state, &params_for(&stranger))
        .expect_err("must reject");
    assert!(err.contains("did not take part"), "unexpected error: {err}");
}

#[test]
fn the_rating_is_the_same_no_matter_what_order_games_arrive_in() {
    let me = key();
    let opponents: Vec<_> = (0..3).map(|_| key()).collect();
    let params = params_for(&me);
    let id = params.player_id();

    let certs: Vec<_> = opponents
        .iter()
        .enumerate()
        .map(|(i, o)| certified_game(o, &me, T0 + i as i64 * 3_600_000, 1200, 1200))
        .collect();

    let mut forward = PlayerStateV1::default();
    for (cert, gid) in &certs {
        forward.history.games.insert(*gid, cert.clone());
    }
    let mut backward = PlayerStateV1::default();
    for (cert, gid) in certs.iter().rev() {
        backward.history.games.insert(*gid, cert.clone());
    }

    forward.verify(&forward, &params).unwrap();
    assert_eq!(
        forward, backward,
        "history is a set, so order cannot matter"
    );
    assert_eq!(
        forward.rating(id),
        backward.rating(id),
        "rating replay must be deterministic"
    );
    assert_eq!(forward.games_played(), 3);
}

#[test]
fn history_merges_idempotently() {
    let me = key();
    let opponent = key();
    let params = params_for(&me);
    let (cert, _) = certified_game(&opponent, &me, T0, 1200, 1200);
    let empty = PlayerStateV1::default();

    let mut state = PlayerStateV1::default();
    state
        .history
        .apply_delta(&empty, &params, &Some(vec![cert.clone()]))
        .unwrap();
    let once = state.clone();
    state
        .history
        .apply_delta(&empty, &params, &Some(vec![cert]))
        .unwrap();
    assert_eq!(once, state, "re-applying a game must change nothing");
    assert_eq!(state.history.len(), 1);
}

#[test]
fn filing_someone_elses_game_is_refused_on_apply() {
    let me = key();
    let a = key();
    let b = key();
    let (cert, _) = certified_game(&a, &b, T0, 1200, 1200);
    let empty = PlayerStateV1::default();

    let mut state = PlayerStateV1::default();
    let err = state
        .history
        .apply_delta(&empty, &params_for(&me), &Some(vec![cert]))
        .expect_err("must refuse");
    assert!(err.contains("did not play it"), "unexpected error: {err}");
}

#[test]
fn the_history_cap_keeps_the_newest_games() {
    let me = key();
    let opponent = key();
    let (base, _) = certified_game(&opponent, &me, T0, 1200, 1200);

    let mut state = PlayerStateV1::default();
    for i in 0..(PROFILE_HISTORY_SIZE + 25) {
        let mut cert = base.clone();
        let mut id = [0u8; 32];
        id[..8].copy_from_slice(&(i as u64).to_le_bytes());
        cert.game_id = crate::identity::GameId(id);
        cert.finished_at = T0 + i as i64 * 1000;
        state.history.games.insert(cert.game_id, cert);
    }

    state.prune(&params_for(&me)).unwrap();
    assert_eq!(state.history.len(), PROFILE_HISTORY_SIZE);
    assert_eq!(
        state.history.games.values().map(|c| c.finished_at).min(),
        Some(T0 + 25 * 1000),
        "the oldest games are the ones dropped"
    );

    // Idempotent.
    let once = state.clone();
    state.prune(&params_for(&me)).unwrap();
    assert_eq!(once, state);
}

#[test]
fn a_new_player_starts_at_the_starting_rating() {
    let me = key();
    let state = PlayerStateV1::default();
    assert_eq!(
        state.rating(params_for(&me).player_id()),
        crate::elo::STARTING_ELO
    );
    assert_eq!(state.games_played(), 0);
}
