//! Deterministic Elo rating.
//!
//! Every peer must compute the *identical* rating from the same set of game
//! results, so this uses integer arithmetic throughout. Floating point would be
//! reproducible in practice on IEEE-754 hardware, but the expected-score
//! formula involves `powf`, whose last-bit behaviour is not guaranteed
//! identical across libm implementations — and a one-point rating disagreement
//! between two peers is a permanent state divergence.
//!
//! The formula is standard Elo with the expected score evaluated from a lookup
//! table indexed by rating difference, which is exactly how FIDE itself
//! defines it.

use serde::{Deserialize, Serialize};

/// Rating every new player starts from.
pub const STARTING_ELO: i32 = 1200;

/// Ratings are clamped to this floor so a losing streak cannot run away.
pub const MIN_ELO: i32 = 100;

/// And to this ceiling. Not a rating anyone reaches — the highest human Elo ever
/// recorded is under 2900 — but a rating arrives from a certificate's
/// `*_rating_before`, which its two signers choose freely. Without a ceiling
/// `rating + delta` can wrap on an absurd input and land *below* the floor,
/// making the clamp that is supposed to bound ratings the thing that breaks
/// them.
pub const MAX_ELO: i32 = 4000;

/// Games played before a rating is considered established. Provisional players
/// move faster (higher K), the way most rating systems bootstrap.
pub const PROVISIONAL_GAMES: u32 = 20;

const K_PROVISIONAL: i32 = 40;
const K_ESTABLISHED: i32 = 20;
/// Established *and* strong: the FIDE-style reduced K for masters.
const K_ELITE: i32 = 10;
const ELITE_THRESHOLD: i32 = 2400;

/// Result of a game from one player's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Score {
    Win,
    Draw,
    Loss,
}

impl Score {
    /// Score in thousandths, so the whole calculation stays integral.
    fn milli(self) -> i32 {
        match self {
            Score::Win => 1000,
            Score::Draw => 500,
            Score::Loss => 0,
        }
    }

    pub fn inverse(self) -> Score {
        match self {
            Score::Win => Score::Loss,
            Score::Draw => Score::Draw,
            Score::Loss => Score::Win,
        }
    }
}

/// Expected score (in thousandths) for a player rated `diff` points above the
/// opponent, from the standard FIDE table. Index is `diff / 25`, capped at 800
/// points where the table saturates.
const EXPECTED_TABLE: [i32; 33] = [
    500, 536, 571, 606, 640, 673, 705, 736, 765, 792, 818, 842, 864, 884, 903, 919, 934, 947, 958,
    967, 975, 981, 986, 990, 993, 995, 997, 998, 999, 999, 1000, 1000, 1000,
];

/// Expected score in thousandths for `rating` against `opponent_rating`.
fn expected_milli(rating: i32, opponent_rating: i32) -> i32 {
    // Saturating, and `unsigned_abs` rather than `abs`: both operands can be any
    // `i32` a certificate's signers chose, so the subtraction can overflow and
    // `i32::MIN.abs()` panics — which under `panic = 'abort'` in WASM is a
    // contract trap on every peer that applies the update.
    let diff = rating.saturating_sub(opponent_rating);
    let magnitude = diff.unsigned_abs().min(800) as i32;
    // Round to nearest table slot rather than truncating, so +/- diffs stay
    // symmetric: expected(a,b) + expected(b,a) == 1000.
    let idx = ((magnitude + 12) / 25) as usize;
    let value = EXPECTED_TABLE[idx.min(EXPECTED_TABLE.len() - 1)];
    if diff >= 0 {
        value
    } else {
        1000 - value
    }
}

/// K-factor for a player, from their rating and experience.
fn k_factor(rating: i32, games_played: u32) -> i32 {
    if games_played < PROVISIONAL_GAMES {
        K_PROVISIONAL
    } else if rating >= ELITE_THRESHOLD {
        K_ELITE
    } else {
        K_ESTABLISHED
    }
}

/// New rating after one game. Pure and integral: identical on every peer.
pub fn apply_result(rating: i32, games_played: u32, opponent_rating: i32, score: Score) -> i32 {
    let k = k_factor(rating, games_played);
    let delta_milli = k * (score.milli() - expected_milli(rating, opponent_rating));
    // Round half away from zero, so a symmetric pair of results is symmetric.
    let delta = if delta_milli >= 0 {
        (delta_milli + 500) / 1000
    } else {
        (delta_milli - 500) / 1000
    };
    rating.saturating_add(delta).clamp(MIN_ELO, MAX_ELO)
}

/// One rated game, as recorded in a player's history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatedGame {
    /// The opponent's rating at the time the game was played, as certified by
    /// both players. Stored rather than looked up so replaying the history
    /// never needs to fetch another contract.
    pub opponent_rating: i32,
    pub score: Score,
}

/// Replay a whole history into a final rating.
///
/// The history is ordered, so this is *not* commutative on its own — the
/// ordering is imposed by the player contract (see [`crate::player`]), which
/// sorts certified results deterministically before calling this.
pub fn rating_from_history(history: &[RatedGame]) -> i32 {
    let mut rating = STARTING_ELO;
    for (i, game) in history.iter().enumerate() {
        rating = apply_result(rating, i as u32, game.opponent_rating, game.score);
    }
    rating
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extreme_ratings_do_not_overflow_or_panic() {
        // Ratings arrive from a certificate's `*_rating_before`, which is
        // attacker-chosen. `rating - opponent_rating` overflows for far-apart
        // values and `i32::MIN.abs()` panics outright — a contract trap in WASM.
        for (rating, opponent) in [
            (i32::MIN, i32::MAX),
            (i32::MAX, i32::MIN),
            (i32::MIN, i32::MIN),
            (i32::MAX, i32::MAX),
            (0, i32::MIN),
            (i32::MIN, 0),
        ] {
            for score in [Score::Win, Score::Draw, Score::Loss] {
                let out = apply_result(rating, 0, opponent, score);
                assert!(out >= MIN_ELO, "rating floor must hold, got {out}");
            }
        }
    }

    #[test]
    fn a_rating_near_the_ceiling_does_not_wrap() {
        // There is a floor but no ceiling, so `rating + delta` could wrap to a
        // huge negative number and land *below* MIN_ELO after the clamp.
        let out = apply_result(i32::MAX, 0, 0, Score::Win);
        assert!((MIN_ELO..=MAX_ELO).contains(&out), "got {out}");
    }

    #[test]
    fn equal_ratings_expect_a_draw() {
        assert_eq!(expected_milli(1500, 1500), 500);
    }

    #[test]
    fn expectations_are_symmetric() {
        // The pair of expected scores must sum to exactly 1.000, otherwise
        // rating points are created or destroyed on every game.
        for diff in (0..900).step_by(7) {
            let a = expected_milli(1500 + diff, 1500);
            let b = expected_milli(1500, 1500 + diff);
            assert_eq!(a + b, 1000, "asymmetry at diff {diff}");
        }
    }

    #[test]
    fn beating_a_stronger_player_gains_more_than_beating_a_weaker_one() {
        let vs_stronger = apply_result(1500, 50, 1900, Score::Win) - 1500;
        let vs_weaker = apply_result(1500, 50, 1100, Score::Win) - 1500;
        assert!(
            vs_stronger > vs_weaker,
            "upset {vs_stronger} should beat easy win {vs_weaker}"
        );
    }

    #[test]
    fn losing_to_a_weaker_player_costs_more() {
        let to_weaker = 1500 - apply_result(1500, 50, 1100, Score::Loss);
        let to_stronger = 1500 - apply_result(1500, 50, 1900, Score::Loss);
        assert!(to_weaker > to_stronger);
    }

    #[test]
    fn provisional_players_move_faster() {
        let provisional = apply_result(1200, 0, 1200, Score::Win) - 1200;
        let established = apply_result(1200, 100, 1200, Score::Win) - 1200;
        assert!(provisional > established);
    }

    #[test]
    fn elite_players_move_slowest() {
        let elite = apply_result(2500, 100, 2500, Score::Win) - 2500;
        let established = apply_result(1500, 100, 1500, Score::Win) - 1500;
        assert!(elite < established);
    }

    #[test]
    fn rating_never_falls_below_the_floor() {
        let mut rating = 200;
        for i in 0..200 {
            rating = apply_result(rating, i, 2000, Score::Loss);
        }
        assert!(rating >= MIN_ELO, "rating {rating} fell through the floor");
    }

    #[test]
    fn a_draw_between_equals_changes_nothing() {
        assert_eq!(apply_result(1500, 50, 1500, Score::Draw), 1500);
    }

    #[test]
    fn history_replay_is_deterministic() {
        let history = vec![
            RatedGame {
                opponent_rating: 1400,
                score: Score::Win,
            },
            RatedGame {
                opponent_rating: 1600,
                score: Score::Loss,
            },
            RatedGame {
                opponent_rating: 1500,
                score: Score::Draw,
            },
        ];
        let a = rating_from_history(&history);
        let b = rating_from_history(&history);
        assert_eq!(a, b);
        assert_ne!(a, STARTING_ELO);
    }

    #[test]
    fn empty_history_is_the_starting_rating() {
        assert_eq!(rating_from_history(&[]), STARTING_ELO);
    }

    #[test]
    fn a_win_and_a_matching_loss_roughly_cancel() {
        // Zero-sum sanity: what one player gains, the other loses, within the
        // rounding tolerance of one point.
        let winner_gain = apply_result(1500, 50, 1500, Score::Win) - 1500;
        let loser_loss = 1500 - apply_result(1500, 50, 1500, Score::Loss);
        assert!((winner_gain - loser_loss).abs() <= 1);
    }
}
