//! Lobby tests: anti-spam quotas, live snapshots, search, ranking, presence.

use super::*;
use crate::game::opponent::{SignedAcceptance, SignedJoin};
use crate::game::WinReason;
use crate::leaderboard::{LeaderboardV1, RankEntry};
use crate::presence::{
    PresenceStatus, SignedPresence, AWAY_WINDOW_MS, MAX_PRESENCE_ENTRIES, ONLINE_WINDOW_MS,
};
use crate::testutil::{certified_game, key, open_game, T0};
use ed25519_dalek::SigningKey;
use freenet_scaffold::ComposableState;

fn params() -> LobbyParametersV1 {
    LobbyParametersV1::default()
}

fn listing(creator: &SigningKey, created_at: i64) -> LobbyEntry {
    let (state, p) = open_game(creator, created_at, "creator");
    LobbyEntry::new(p.game_id, state.setup.0.clone().unwrap())
}

/// A listing that already found a challenger.
fn matched_listing(creator: &SigningKey, challenger: &SigningKey, created_at: i64) -> LobbyEntry {
    let (state, p) = open_game(creator, created_at, "creator");
    let mut entry = LobbyEntry::new(p.game_id, state.setup.0.clone().unwrap());
    entry.opponent = Some(SignedAcceptance::new(
        creator,
        &p.game_id,
        SignedJoin::new(
            challenger,
            &p.game_id,
            created_at + 500,
            "challenger".to_string(),
        ),
    ));
    entry
}

fn lobby_with(entries: Vec<LobbyEntry>) -> LobbyStateV1 {
    let mut state = LobbyStateV1::default();
    for e in entries {
        state.games.games.insert(e.game_id, e);
    }
    state
}

// ------------------------------------------------------------------ anti-spam

#[test]
fn a_creator_cannot_hold_more_than_the_open_game_quota() {
    let creator = key();
    // Ten open challenges from one key — the classic lobby flood.
    let entries: Vec<LobbyEntry> = (0..10).map(|i| listing(&creator, T0 + i * 1000)).collect();
    let mut state = lobby_with(entries);
    assert_eq!(state.games.len(), 10);

    state.prune(&params()).unwrap();

    assert_eq!(
        state.games.len(),
        MAX_OPEN_PER_CREATOR,
        "quota must cap open games per creator"
    );
    // The survivors are the newest three, chosen deterministically.
    let mut kept: Vec<i64> = state.games.games.values().map(|e| e.created_at()).collect();
    kept.sort();
    assert_eq!(kept, vec![T0 + 7000, T0 + 8000, T0 + 9000]);
}

#[test]
fn the_quota_is_per_creator_not_global() {
    // Three players each opening their limit must all be kept — the anti-spam
    // rule must not punish a busy lobby.
    let creators: Vec<SigningKey> = (0..3).map(|_| key()).collect();
    let mut entries = Vec::new();
    for creator in &creators {
        for i in 0..MAX_OPEN_PER_CREATOR as i64 {
            entries.push(listing(creator, T0 + i * 1000));
        }
    }
    let mut state = lobby_with(entries);
    state.prune(&params()).unwrap();
    assert_eq!(state.games.len(), 3 * MAX_OPEN_PER_CREATOR);
}

#[test]
fn games_that_found_an_opponent_do_not_count_against_the_open_quota() {
    let creator = key();
    let challenger = key();
    let entries: Vec<LobbyEntry> = (0..5)
        .map(|i| matched_listing(&creator, &challenger, T0 + i * 1000))
        .collect();
    let mut state = lobby_with(entries);
    state.prune(&params()).unwrap();
    assert_eq!(state.games.len(), 5, "running games must not be evicted");
}

#[test]
fn quota_enforcement_is_idempotent_and_order_independent() {
    let creator = key();
    let entries: Vec<LobbyEntry> = (0..8).map(|i| listing(&creator, T0 + i * 1000)).collect();

    let mut peer1 = lobby_with(entries.clone());
    let mut reversed = entries;
    reversed.reverse();
    let mut peer2 = lobby_with(reversed);

    peer1.prune(&params()).unwrap();
    peer2.prune(&params()).unwrap();
    assert_eq!(peer1, peer2, "eviction must not depend on arrival order");

    let once = peer1.clone();
    peer1.prune(&params()).unwrap();
    assert_eq!(once, peer1, "eviction must be idempotent");
}

#[test]
fn a_full_state_put_that_violates_the_quota_is_rejected() {
    // The merge path prunes, but a full-state PUT skips it, so `verify` has to
    // catch a flooded lobby on its own.
    let creator = key();
    let entries: Vec<LobbyEntry> = (0..10).map(|i| listing(&creator, T0 + i * 1000)).collect();
    let state = lobby_with(entries);
    let err = state.verify(&state, &params()).expect_err("must reject");
    assert!(err.contains("open games"), "unexpected error: {err}");
}

#[test]
fn an_entry_with_a_forged_setup_is_rejected() {
    let creator = key();
    let impostor = key();
    let mut entry = listing(&creator, T0);
    entry.setup.creator = impostor.verifying_key();
    let state = lobby_with(vec![entry]);
    assert!(state.verify(&state, &params()).is_err());
}

#[test]
fn a_valid_lobby_verifies() {
    let creator = key();
    let challenger = key();
    let state = lobby_with(vec![
        listing(&creator, T0),
        matched_listing(&creator, &challenger, T0 + 5000),
    ]);
    state
        .verify(&state, &params())
        .expect("honest lobby is valid");
}

// ----------------------------------------------------------- live snapshots

#[test]
fn only_a_player_of_that_game_may_publish_its_snapshot() {
    let creator = key();
    let challenger = key();
    let outsider = key();
    let mut entry = matched_listing(&creator, &challenger, T0);
    let game_id = entry.game_id;

    // A stranger must not be able to rewrite what the home page shows.
    entry.snapshot = Some(SignedSnapshot::new(
        &outsider,
        &game_id,
        crate::chess::STARTING_FEN.to_string(),
        0,
        GameResult::InProgress,
        600_000,
        600_000,
        T0 + 1000,
    ));
    let state = lobby_with(vec![entry.clone()]);
    let err = state.verify(&state, &params()).expect_err("must reject");
    assert!(err.contains("not a player in this game"), "got: {err}");

    // Either real player may.
    for signer in [&creator, &challenger] {
        entry.snapshot = Some(SignedSnapshot::new(
            signer,
            &game_id,
            crate::chess::STARTING_FEN.to_string(),
            0,
            GameResult::InProgress,
            600_000,
            600_000,
            T0 + 1000,
        ));
        let state = lobby_with(vec![entry.clone()]);
        state
            .verify(&state, &params())
            .expect("a player may publish");
    }
}

#[test]
fn the_newest_snapshot_wins_regardless_of_merge_order() {
    let creator = key();
    let challenger = key();
    let base = matched_listing(&creator, &challenger, T0);
    let game_id = base.game_id;

    let mut early = base.clone();
    early.snapshot = Some(SignedSnapshot::new(
        &creator,
        &game_id,
        "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1".to_string(),
        1,
        GameResult::InProgress,
        599_000,
        600_000,
        T0 + 2000,
    ));

    let mut late = base.clone();
    late.snapshot = Some(SignedSnapshot::new(
        &challenger,
        &game_id,
        "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq e6 0 2".to_string(),
        2,
        GameResult::InProgress,
        599_000,
        598_000,
        T0 + 3000,
    ));

    let empty = LobbyStateV1::default();
    let mut peer1 = lobby_with(vec![base.clone()]);
    peer1
        .games
        .apply_delta(&empty, &params(), &Some(vec![early.clone(), late.clone()]))
        .unwrap();
    let mut peer2 = lobby_with(vec![base]);
    peer2
        .games
        .apply_delta(&empty, &params(), &Some(vec![late, early]))
        .unwrap();

    assert_eq!(peer1, peer2, "snapshots must converge");
    assert_eq!(
        peer1
            .games
            .get(&game_id)
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .ply,
        2,
        "the later position must win"
    );
}

#[test]
fn live_and_open_lists_separate_correctly() {
    let creator = key();
    let challenger = key();
    let live = matched_listing(&creator, &challenger, T0 + 5000);
    let state = lobby_with(vec![listing(&creator, T0), live.clone()]);

    assert_eq!(state.games.open_games().len(), 1);
    assert_eq!(state.games.live_games().len(), 1);
    assert_eq!(state.games.live_games()[0].game_id, live.game_id);
}

#[test]
fn a_finished_game_is_no_longer_live() {
    let creator = key();
    let challenger = key();
    let mut entry = matched_listing(&creator, &challenger, T0);
    let game_id = entry.game_id;
    entry.snapshot = Some(SignedSnapshot::new(
        &creator,
        &game_id,
        crate::chess::STARTING_FEN.to_string(),
        4,
        GameResult::BlackWins(WinReason::Checkmate),
        500_000,
        500_000,
        T0 + 9000,
    ));
    let state = lobby_with(vec![entry]);
    assert!(state.games.live_games().is_empty());
}

// ---------------------------------------------------------------- searching

#[test]
fn search_matches_nicknames_and_game_id_prefixes() {
    let creator = key();
    let (game, p) = open_game(&creator, T0, "magnus");
    let lobby = lobby_with(vec![LobbyEntry::new(
        p.game_id,
        game.setup.0.clone().unwrap(),
    )]);

    assert_eq!(lobby.games.search("magnus").len(), 1);
    assert_eq!(
        lobby.games.search("MAG").len(),
        1,
        "search is case-insensitive"
    );
    assert_eq!(lobby.games.search("hikaru").len(), 0);
    assert_eq!(
        lobby.games.search(&p.game_id.to_base58()[..6]).len(),
        1,
        "a game-id prefix should match"
    );
    assert!(
        lobby.games.search("   ").is_empty(),
        "a blank query matches nothing"
    );
}

// -------------------------------------------------------------- leaderboard

/// A rank entry for `player`, backed by a real fool's-mate game they won as
/// Black.
fn winning_rank_entry(player: &SigningKey, loser: &SigningKey, at: i64) -> RankEntry {
    let (cert, _) = certified_game(loser, player, at, 1200, 1200);
    let me = crate::identity::PlayerId::from(&player.verifying_key());
    let rating = crate::elo::apply_result(
        1200,
        0,
        cert.opponent_rating_for(me).unwrap(),
        cert.score_for(me).unwrap(),
    );
    RankEntry::new(player, "player".to_string(), rating, 1, cert, at + 61_000)
}

#[test]
fn a_rank_entry_backed_by_a_real_game_verifies() {
    let winner = key();
    let loser = key();
    let entry = winning_rank_entry(&winner, &loser, T0);
    entry.verify().expect("an honest rank entry must verify");
    assert!(entry.rating > 1200, "the winner's rating should have risen");
}

#[test]
fn an_inflated_rating_is_rejected() {
    let winner = key();
    let loser = key();
    let honest = winning_rank_entry(&winner, &loser, T0);

    // Claim a grandmaster rating and re-sign it, so only the arithmetic check
    // stands between the lie and the ranking.
    let forged = RankEntry::new(
        &winner,
        honest.nickname.clone(),
        2800,
        honest.games_played,
        honest.last_game.clone(),
        honest.updated_at,
    );
    let err = forged.verify().expect_err("must reject");
    assert!(err.contains("does not follow"), "unexpected error: {err}");
}

#[test]
fn a_rank_entry_citing_someone_elses_game_is_rejected() {
    let stranger = key();
    let a = key();
    let b = key();
    let (cert, _) = certified_game(&a, &b, T0, 1200, 1200);

    let entry = RankEntry::new(&stranger, "leech".to_string(), 1216, 1, cert, T0 + 61_000);
    let err = entry.verify().expect_err("must reject");
    assert!(err.contains("did not take part"), "unexpected error: {err}");
}

#[test]
fn the_ranking_is_ordered_by_rating_and_converges() {
    let mut board = LeaderboardV1::default();
    let mut entries = Vec::new();
    for _ in 0..3 {
        let winner = key();
        let loser = key();
        entries.push(winning_rank_entry(&winner, &loser, T0));
    }
    for e in &entries {
        board.entries.insert(e.player_id(), e.clone());
    }

    let ranked = board.ranked();
    assert_eq!(ranked.len(), 3);
    for pair in ranked.windows(2) {
        assert!(pair[0].rating >= pair[1].rating, "must sort by rating");
    }

    // Inserting in the opposite order gives the identical table.
    let mut other = LeaderboardV1::default();
    for e in entries.iter().rev() {
        other.entries.insert(e.player_id(), e.clone());
    }
    assert_eq!(board, other);
}

// ----------------------------------------------------------------- presence

#[test]
fn presence_decays_from_online_to_away_to_offline_without_any_write() {
    let player = key();
    let beat = SignedPresence::new(&player, "player".to_string(), PresenceStatus::Online, T0);
    beat.verify().expect("heartbeat must verify");

    assert_eq!(beat.status_at(T0), PresenceStatus::Online);
    assert_eq!(
        beat.status_at(T0 + ONLINE_WINDOW_MS - 1),
        PresenceStatus::Online
    );
    // Just past the online window it is away, and past the away window it is
    // offline — all from the passage of time, with no further updates.
    assert_eq!(
        beat.status_at(T0 + ONLINE_WINDOW_MS + 1),
        PresenceStatus::Away
    );
    assert_eq!(
        beat.status_at(T0 + AWAY_WINDOW_MS + 1),
        PresenceStatus::Offline
    );
}

#[test]
fn an_explicit_away_claim_is_honoured_even_when_fresh() {
    let player = key();
    let beat = SignedPresence::new(&player, "player".to_string(), PresenceStatus::Away, T0);
    assert_eq!(beat.status_at(T0), PresenceStatus::Away);
}

#[test]
fn nobody_can_forge_another_players_presence() {
    let player = key();
    let impostor = key();
    let mut beat = SignedPresence::new(&player, "player".to_string(), PresenceStatus::Online, T0);
    // Swap in someone else's identity, keeping the valid signature.
    beat.player = impostor.verifying_key();
    assert!(beat.verify().is_err());
}

#[test]
fn the_newest_heartbeat_wins_in_any_order() {
    let player = key();
    let old = SignedPresence::new(&player, "player".to_string(), PresenceStatus::Online, T0);
    let new = SignedPresence::new(
        &player,
        "player".to_string(),
        PresenceStatus::Online,
        T0 + 30_000,
    );
    let empty = LobbyStateV1::default();

    let mut peer1 = LobbyStateV1::default();
    peer1
        .presence
        .apply_delta(&empty, &params(), &Some(vec![old.clone(), new.clone()]))
        .unwrap();
    let mut peer2 = LobbyStateV1::default();
    peer2
        .presence
        .apply_delta(&empty, &params(), &Some(vec![new, old]))
        .unwrap();

    assert_eq!(peer1.presence, peer2.presence);
    let id = crate::identity::PlayerId::from(&player.verifying_key());
    assert_eq!(peer1.presence.players[&id].updated_at, T0 + 30_000);
}

#[test]
fn a_player_with_no_heartbeat_reads_as_offline() {
    let state = LobbyStateV1::default();
    let id = crate::identity::PlayerId::from(&key().verifying_key());
    assert_eq!(state.presence.status_of(id, T0), PresenceStatus::Offline);
}

#[test]
fn the_active_list_excludes_expired_players() {
    let fresh = key();
    let stale = key();
    let mut state = LobbyStateV1::default();
    for (k, at) in [(&fresh, T0), (&stale, T0 - AWAY_WINDOW_MS - 60_000)] {
        let beat = SignedPresence::new(k, "p".to_string(), PresenceStatus::Online, at);
        state.presence.players.insert(beat.player_id(), beat);
    }

    let active = state.presence.active(T0);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].player, fresh.verifying_key());
    assert_eq!(state.presence.online_count(T0), 1);
}

/// A lobby pushed past [`MAX_LOBBY_ENTRIES`] by one flooder holding many
/// keypairs, plus one honest player who got there first.
///
/// Eviction reads only creator, timestamps and open/matched status, so each
/// creator's listings are cloned from one mined setup under distinct ids rather
/// than mining a fresh proof for all 500-odd. That keeps the fixture cheap; the
/// proof-of-work binding itself is covered by the `verify` tests above.
///
/// Cloning is also the honest model of the attack: the flooder's real cost is
/// one proof per listing, which is only `MAX_LOBBY_ENTRIES` proofs to cover the
/// whole lobby -- seconds of one core.
fn flooded_corpus() -> (Vec<LobbyEntry>, PlayerId) {
    let challenger = key();
    let honest = key();
    let honest_id = PlayerId::from(&honest.verifying_key());

    // Distinct ids for cloned listings, so each occupies its own lobby slot.
    let mut next_id = 0u64;
    let mut spread = |base: &LobbyEntry, n: usize| -> Vec<LobbyEntry> {
        (0..n)
            .map(|_| {
                next_id += 1;
                let mut e = base.clone();
                let mut raw = [0u8; 32];
                raw[..8].copy_from_slice(&next_id.to_le_bytes());
                e.game_id = GameId(raw);
                e
            })
            .collect()
    };

    // The honest player got there first, so every flood listing is newer.
    let mut entries = spread(&matched_listing(&honest, &challenger, T0), 1);
    for k in 0..(MAX_LOBBY_ENTRIES / MAX_LISTINGS_PER_CREATOR) as i64 {
        let flooder = key();
        let base = matched_listing(&flooder, &challenger, T0 + 1_000 + k * 1_000);
        entries.extend(spread(&base, MAX_LISTINGS_PER_CREATOR));
    }
    assert!(
        entries.len() > MAX_LOBBY_ENTRIES,
        "the cap must be exercised"
    );
    (entries, honest_id)
}

#[test]
fn a_flooder_holding_many_keypairs_cannot_evict_an_honest_players_game() {
    // Keypairs are free, so the per-creator quota alone does not bound how much
    // of the lobby one party can occupy: MAX_LOBBY_ENTRIES/MAX_LISTINGS_PER_CREATOR
    // fresh keys cover every slot, at MAX_LOBBY_ENTRIES proofs of work in total.
    //
    // While the global cap kept simply the most recently active games, a flooder
    // whose listings were newer won that truncation outright and evicted real
    // players. Sharing the slots out across creators is what bounds them.
    let (entries, honest_id) = flooded_corpus();
    let mut state = lobby_with(entries);
    state.prune(&params()).unwrap();

    assert_eq!(state.games.len(), MAX_LOBBY_ENTRIES);
    assert!(
        state
            .games
            .games
            .values()
            .any(|e| e.creator_id() == honest_id),
        "an honest player's game was evicted by a flooder holding fresh keypairs"
    );
}

#[test]
fn the_global_cap_is_idempotent_and_independent_of_arrival_order() {
    // The cap is load-bearing for convergence: it must be a pure function of the
    // merged set, so two peers that saw the same games in different orders keep
    // the same ones.
    let (entries, _) = flooded_corpus();

    let mut forwards = lobby_with(entries.clone());
    forwards.prune(&params()).unwrap();

    let mut backwards = lobby_with(entries.iter().rev().cloned().collect());
    backwards.prune(&params()).unwrap();
    assert_eq!(
        forwards.games.games.keys().collect::<Vec<_>>(),
        backwards.games.games.keys().collect::<Vec<_>>(),
        "arrival order changed which games survived the cap"
    );

    // Pruning an already-pruned lobby must change nothing.
    let once = forwards.games.games.clone();
    forwards.prune(&params()).unwrap();
    assert_eq!(forwards.games.games, once, "prune is not idempotent");
}

// ------------------------------------------------- summaries vs merge order

#[test]
fn presence_entries_differing_only_below_the_summary_still_exchange() {
    // `absorb` settles a tie on `updated_at` by signature bytes, but the
    // summary carried only `updated_at`. Two peers holding different heartbeats
    // stamped the same millisecond therefore reported identical summaries,
    // neither shipped a delta, and the tiebreak was unreachable.
    let player = key();
    let a =
        SignedPresence::watching_game(&player, "one".to_string(), PresenceStatus::Online, None, T0);
    let b =
        SignedPresence::watching_game(&player, "two".to_string(), PresenceStatus::Online, None, T0);
    assert_ne!(a.signature, b.signature, "test premise: distinct entries");

    // Which of the two wins depends on random signature bytes, so sync both
    // directions and assert the property that matters: they end up agreeing.
    let winner = if a.signature.to_bytes() > b.signature.to_bytes() {
        a.clone()
    } else {
        b.clone()
    };

    let mut peer1 = LobbyStateV1::default();
    peer1.presence.players.insert(a.player_id(), a);
    let mut peer2 = LobbyStateV1::default();
    peer2.presence.players.insert(b.player_id(), b);

    for _ in 0..2 {
        let s1 = peer1.presence.summarize(&peer1, &params());
        if let Some(d) = peer2.presence.delta(&peer2, &params(), &s1) {
            peer1
                .presence
                .apply_delta(&peer1.clone(), &params(), &Some(d))
                .unwrap();
        }
        let s2 = peer2.presence.summarize(&peer2, &params());
        if let Some(d) = peer1.presence.delta(&peer1, &params(), &s2) {
            peer2
                .presence
                .apply_delta(&peer2.clone(), &params(), &Some(d))
                .unwrap();
        }
    }

    assert_eq!(peer1.presence, peer2.presence);
    assert_eq!(
        peer1.presence.players.get(&winner.player_id()),
        Some(&winner),
        "and settle on the entry the total order picks"
    );
}

#[test]
fn rank_entries_with_equal_games_played_still_exchange() {
    // `order_key` is (games_played, updated_at, signature) but the summary was
    // games_played alone, so a later entry with the same count never shipped.
    let player = key();
    let other = key();
    let (cert, _) = certified_game(&player, &other, T0, 1200, 1200);

    // The rating has to follow from the cited game, so both entries carry the
    // same (correct) one and differ only in `updated_at` — a field the old
    // summary did not carry.
    let me = PlayerId::from(&player.verifying_key());
    let games_played = 4u32;
    let rating = crate::elo::apply_result(
        1200,
        games_played - 1,
        cert.opponent_rating_for(me).expect("player is in the game"),
        cert.score_for(me).expect("game has a result"),
    );
    let stale = RankEntry::new(
        &player,
        "me".to_string(),
        rating,
        games_played,
        cert.clone(),
        T0 + 1_000,
    );
    let fresh = RankEntry::new(
        &player,
        "me".to_string(),
        rating,
        games_played,
        cert,
        T0 + 9_000,
    );

    let mut peer1 = LobbyStateV1::default();
    peer1.leaderboard.entries.insert(stale.player_id(), stale);
    let peer2 = {
        let mut s = LobbyStateV1::default();
        s.leaderboard.entries.insert(fresh.player_id(), fresh);
        s
    };

    let summary = peer1.leaderboard.summarize(&peer1, &params());
    let delta = peer2
        .leaderboard
        .delta(&peer2, &params(), &summary)
        .expect("the later entry must ship");
    peer1
        .leaderboard
        .apply_delta(&peer1.clone(), &params(), &Some(delta))
        .unwrap();

    assert_eq!(peer1.leaderboard, peer2.leaderboard);
}

#[test]
fn snapshots_at_the_same_ply_still_exchange() {
    // The summary's "revision" was `ply + 1 + opponent.is_some()`, but
    // `SignedSnapshot::order_key` is (ply, updated_at, signature). Two peers
    // holding different snapshots of the same ply — both players publishing
    // after the same move, which is the normal case — had equal revisions, so
    // neither shipped and the two lobbies showed different clocks for ever.
    let creator = key();
    let challenger = key();
    let base = matched_listing(&creator, &challenger, T0);
    let game_id = base.game_id;

    let fen = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1".to_string();
    let mut peer1 = lobby_with(vec![{
        let mut e = base.clone();
        e.snapshot = Some(SignedSnapshot::new(
            &creator,
            &game_id,
            fen.clone(),
            1,
            GameResult::InProgress,
            599_000,
            600_000,
            T0 + 2_000,
        ));
        e
    }]);
    let mut peer2 = lobby_with(vec![{
        let mut e = base.clone();
        e.snapshot = Some(SignedSnapshot::new(
            &challenger,
            &game_id,
            fen,
            1,
            GameResult::InProgress,
            598_000,
            600_000,
            T0 + 8_000,
        ));
        e
    }]);
    assert_ne!(peer1.games, peer2.games, "test premise: they differ");

    for _ in 0..2 {
        let s1 = peer1.games.summarize(&peer1, &params());
        if let Some(d) = peer2.games.delta(&peer2, &params(), &s1) {
            peer1
                .games
                .apply_delta(&peer1.clone(), &params(), &Some(d))
                .unwrap();
        }
        let s2 = peer2.games.summarize(&peer2, &params());
        if let Some(d) = peer1.games.delta(&peer1, &params(), &s2) {
            peer2
                .games
                .apply_delta(&peer2.clone(), &params(), &Some(d))
                .unwrap();
        }
    }

    assert_eq!(peer1.games, peer2.games);
    // The later snapshot is the one they agree on.
    assert_eq!(
        peer1
            .games
            .games
            .get(&game_id)
            .and_then(|e| e.snapshot.as_ref())
            .map(|s| s.updated_at),
        Some(T0 + 8_000)
    );
}

// ------------------------------------------------- summary size and bytes

/// Encode with the same codec the contract's `summarize_state` uses.
fn encoded(value: &impl serde::Serialize) -> Vec<u8> {
    let mut buf = Vec::new();
    ciborium::into_writer(value, &mut buf).expect("summary encodes");
    buf
}

fn lobby_at_presence_cap() -> LobbyStateV1 {
    let mut state = LobbyStateV1::default();
    for i in 0..MAX_PRESENCE_ENTRIES {
        let k = key();
        let p = SignedPresence::watching_game(
            &k,
            format!("player{i}"),
            PresenceStatus::Online,
            None,
            T0 + i as i64,
        );
        state.presence.players.insert(p.player_id(), p);
    }
    state
}

#[test]
fn the_presence_summary_stays_delta_efficient_at_the_cap() {
    // A summary exists so peers can detect divergence *without* shipping state,
    // and it rides both directions on every anti-entropy round. freenet-core's
    // own heuristic for whether that is worth it is `summary * 2 < state`.
    // Carrying raw 64-byte signatures put this at 82KB against a 107KB state,
    // which fails outright; a digest of the same bytes distinguishes just as
    // much for a quarter of the size.
    let state = lobby_at_presence_cap();
    let summary = encoded(&state.presence.summarize(&state, &params()));
    let full = encoded(&state.presence);
    assert!(
        summary.len() * 2 < full.len(),
        "presence summary is {} bytes against a {} byte state; a summary that big \
         is not worth sending",
        summary.len(),
        full.len()
    );
}

#[test]
fn summaries_are_byte_identical_regardless_of_insertion_order() {
    // freenet-core decides a peer is stale by *byte-comparing* the output of
    // `summarize_state`. A collection with non-deterministic iteration order
    // (a `HashMap`, or an unsorted `Vec`) therefore makes two fully converged
    // peers look permanently out of sync, and each ~5-minute heartbeat fires a
    // pointless full-state heal. This guards every summary the lobby emits.
    let creator = key();
    let challenger = key();
    let entries: Vec<LobbyEntry> = (0..12)
        .map(|i| {
            if i % 2 == 0 {
                listing(&creator, T0 + i * 1000)
            } else {
                matched_listing(&creator, &challenger, T0 + i * 1000)
            }
        })
        .collect();
    let presences: Vec<SignedPresence> = (0..12)
        .map(|i| {
            SignedPresence::watching_game(
                &key(),
                format!("p{i}"),
                PresenceStatus::Online,
                None,
                T0 + i,
            )
        })
        .collect();

    let build = |reversed: bool| {
        let mut state = LobbyStateV1::default();
        let mut entries = entries.clone();
        let mut presences = presences.clone();
        if reversed {
            entries.reverse();
            presences.reverse();
        }
        for e in entries {
            state.games.games.insert(e.game_id, e);
        }
        for p in presences {
            state.presence.players.insert(p.player_id(), p);
        }
        state
    };

    let forwards = build(false);
    let backwards = build(true);
    assert_eq!(forwards, backwards, "the two states must be the same state");
    assert_eq!(
        encoded(&forwards.summarize(&forwards, &params())),
        encoded(&backwards.summarize(&backwards, &params())),
        "the same logical state must summarize to the same bytes"
    );
}
