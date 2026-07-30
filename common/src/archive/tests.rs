//! Archive tests: shard derivation, the per-shard cap, and replayability.

use super::*;
use crate::testutil::{certified_game, key, T0};
use freenet_scaffold::ComposableState;

fn shard_params(shard: ShardId) -> ArchiveParametersV1 {
    ArchiveParametersV1::new(shard)
}

#[test]
fn a_games_shard_is_derived_from_the_game_itself() {
    let a = key();
    let b = key();
    let (cert, _) = certified_game(&a, &b, T0, 1200, 1200);

    // Any peer computes the same location with no coordination.
    let shard = ShardId::for_game(cert.finished_at, &cert.game_id);
    assert_eq!(shard, ShardId::for_game(cert.finished_at, &cert.game_id));
    assert!(shard.bucket < BUCKETS);
    assert_eq!(shard.day, ShardId::day_of(cert.finished_at));
}

#[test]
fn a_day_is_covered_by_exactly_the_bucket_set() {
    let buckets = ShardId::buckets_for_day(20_000);
    assert_eq!(buckets.len(), BUCKETS as usize);
    for (i, shard) in buckets.iter().enumerate() {
        assert_eq!(shard.bucket, i as u8);
        assert_eq!(shard.day, 20_000);
    }
}

#[test]
fn an_archived_game_verifies_and_replays() {
    let white = key();
    let black = key();
    let (cert, game_id) = certified_game(&white, &black, T0, 1200, 1200);
    let shard = ShardId::for_game(cert.finished_at, &game_id);

    let mut state = ArchiveStateV1::default();
    state.games.games.insert(game_id, cert.clone());
    state
        .verify(&state, &shard_params(shard))
        .expect("archived game must verify");

    // The whole point of the archive: the game replays from it.
    let replayed = state.games.get(&game_id).unwrap().replay();
    assert_eq!(replayed.san_list(), vec!["f3", "e5", "g4", "Qh4#"]);
    assert!(cert.to_pgn().contains("1. f3 e5 2. g4 Qh4#"));
}

#[test]
fn a_game_filed_in_the_wrong_shard_is_rejected() {
    let a = key();
    let b = key();
    let (cert, game_id) = certified_game(&a, &b, T0, 1200, 1200);
    let correct = ShardId::for_game(cert.finished_at, &game_id);
    let wrong = ShardId {
        day: correct.day + 1,
        bucket: correct.bucket,
    };

    let mut state = ArchiveStateV1::default();
    state.games.games.insert(game_id, cert);
    let err = state
        .verify(&state, &shard_params(wrong))
        .expect_err("must reject");
    assert!(err.contains("belongs in shard"), "unexpected error: {err}");
}

#[test]
fn a_tampered_certificate_is_rejected() {
    let a = key();
    let b = key();
    let (mut cert, game_id) = certified_game(&a, &b, T0, 1200, 1200);
    let shard = ShardId::for_game(cert.finished_at, &game_id);

    // Flip the result: both signatures now cover different bytes.
    cert.result = crate::game::GameResult::WhiteWins(crate::game::WinReason::Checkmate);
    let mut state = ArchiveStateV1::default();
    state.games.games.insert(game_id, cert);
    assert!(state.verify(&state, &shard_params(shard)).is_err());
}

#[test]
fn the_shard_cap_keeps_the_newest_games_deterministically() {
    // Build more certificates than the cap, cheaply, by cloning one valid
    // certificate under fresh ids and finish times. Signatures would not check
    // out, so this exercises `enforce_cap` directly rather than `verify`.
    let a = key();
    let b = key();
    let (base, _) = certified_game(&a, &b, T0, 1200, 1200);

    let mut games = ArchivedGamesV1::default();
    for i in 0..(GAMES_PER_SHARD + 50) {
        let mut cert = base.clone();
        let mut id = [0u8; 32];
        id[..8].copy_from_slice(&(i as u64).to_le_bytes());
        cert.game_id = crate::identity::GameId(id);
        cert.finished_at = T0 + i as i64 * 1000;
        games.games.insert(cert.game_id, cert);
    }
    assert_eq!(games.len(), GAMES_PER_SHARD + 50);

    let mut forward = games.clone();
    forward.enforce_cap();
    assert_eq!(forward.len(), GAMES_PER_SHARD);

    // The oldest 50 are the ones dropped.
    let oldest_kept = forward.games.values().map(|c| c.finished_at).min().unwrap();
    assert_eq!(oldest_kept, T0 + 50 * 1000);

    // Idempotent, and independent of insertion order.
    let twice = {
        let mut g = forward.clone();
        g.enforce_cap();
        g
    };
    assert_eq!(forward, twice, "cap must be idempotent");
}

#[test]
fn archives_converge_regardless_of_delta_order() {
    let a = key();
    let b = key();
    let c = key();
    let (cert1, _) = certified_game(&a, &b, T0, 1200, 1200);
    // A second game the same day, so both land on the same day (buckets may
    // differ, but the merge logic is what is under test here).
    let (cert2, _) = certified_game(&a, &c, T0 + 3_600_000, 1200, 1200);

    let empty = ArchiveStateV1::default();
    let shard = ShardId::for_game(cert1.finished_at, &cert1.game_id);
    let params = shard_params(shard);

    // Only certificates belonging to this shard are accepted, so filter to the
    // ones that do and check both orders agree.
    let mine: Vec<_> = [cert1, cert2]
        .into_iter()
        .filter(|c| ShardId::for_game(c.finished_at, &c.game_id) == shard)
        .collect();

    let mut peer1 = ArchiveStateV1::default();
    peer1
        .games
        .apply_delta(&empty, &params, &Some(mine.clone()))
        .unwrap();
    let mut reversed = mine;
    reversed.reverse();
    let mut peer2 = ArchiveStateV1::default();
    peer2
        .games
        .apply_delta(&empty, &params, &Some(reversed.clone()))
        .unwrap();

    assert_eq!(peer1, peer2, "archive must converge");

    // Re-applying changes nothing.
    let before = peer1.clone();
    peer1
        .games
        .apply_delta(&empty, &params, &Some(reversed))
        .unwrap();
    assert_eq!(before, peer1, "archive merge must be idempotent");
}

#[test]
fn a_certificate_from_another_shard_is_refused_on_apply() {
    let a = key();
    let b = key();
    let (cert, game_id) = certified_game(&a, &b, T0, 1200, 1200);
    let correct = ShardId::for_game(cert.finished_at, &game_id);
    let wrong = ShardId {
        day: correct.day,
        bucket: (correct.bucket + 1) % BUCKETS,
    };

    let empty = ArchiveStateV1::default();
    let mut state = ArchiveStateV1::default();
    let err = state
        .games
        .apply_delta(&empty, &shard_params(wrong), &Some(vec![cert]))
        .expect_err("must refuse");
    assert!(err.contains("does not belong"), "unexpected error: {err}");
}

#[test]
fn searching_finds_games_by_nickname_and_player() {
    let white = key();
    let black = key();
    let (cert, game_id) = certified_game(&white, &black, T0, 1200, 1200);
    let mut games = ArchivedGamesV1::default();
    games.games.insert(game_id, cert.clone());

    // The fixture names the creator "creator" and the challenger "opponent".
    assert_eq!(games.search("creator").len(), 1);
    assert_eq!(games.search("opponent").len(), 1);
    assert_eq!(games.search("nobody").len(), 0);
    assert_eq!(games.games_of(cert.white_id()).len(), 1);
    assert_eq!(games.games_of(cert.black_id()).len(), 1);
    let stranger = crate::identity::PlayerId::from(&key().verifying_key());
    assert_eq!(games.games_of(stranger).len(), 0);
}
