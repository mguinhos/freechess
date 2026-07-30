//! The co-signed record of a finished game.
//!
//! A certificate is the compact, self-verifying artifact that outlives the game
//! contract. It carries the full move list, so a replay reconstructs the game
//! exactly, plus both players' signatures over the outcome.
//!
//! # Why a certificate instead of archiving the game state
//!
//! A game's own contract state is authoritative but bulky: every move carries a
//! 64-byte signature and a 32-byte key, so a long game runs to ~10 KB. A
//! thousand of those in one archive shard would be ~10 MB of state to
//! replicate. The certificate stores each move in 3 bytes and signs the whole
//! list twice, landing at a few hundred bytes — a thousand games fit in well
//! under a megabyte, while still being verifiable from the bytes alone.
//!
//! # What it proves, and what it does not
//!
//! Verification re-plays the move list through the engine, so the moves are
//! provably legal and a board result (checkmate, stalemate, the draw rules) is
//! provably correct. Results that happen off the board — resignation, an agreed
//! draw, a flag fall — rest on both players having signed, which is exactly the
//! authority that decides them.
//!
//! Two colluding keys can still co-sign a fabricated result between themselves.
//! That is inherent: the whitepaper places Sybil resistance at the application
//! layer, and no purely local check distinguishes a real game between two
//! willing players from a staged one. The proof-of-work on game creation prices
//! the attack, and ratings are only as meaningful as the opponents faced.

use crate::chess::{Color, Game, GameStatus, Move};
use crate::elo::{MAX_ELO, MIN_ELO};
use crate::game::setup::{leading_zero_bits, TimeControl, MAX_NICKNAME_LEN, POW_DIFFICULTY_BITS};
use crate::game::{ChessGameStateV1, DrawReason, GameResult, WinReason, SIG_DOMAIN};
use crate::identity::{verify_sig, GameId, PlayerId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Longest game a certificate will carry. Well past the ~5900-ply theoretical
/// maximum of a legal game under the 50-move rule in practice, while bounding
/// how large a single archive entry can get.
pub const MAX_CERTIFIED_PLIES: usize = 1024;

/// Upper bound on a certificate's timestamps: 2100-01-01 in Unix milliseconds.
///
/// A self-asserted timestamp that is merely *large* is not harmless here.
/// `finished_at` chooses a game's archive shard and orders it inside that shard,
/// so a far-future value both spawns shard contracts at absurd epochs and, under
/// newest-first eviction, pushes real games out of the shard it lands in.
pub const MAX_CERTIFICATE_TIME_MS: i64 = 4_102_444_800_000;

/// The two players' agreed record of a finished game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameCertificate {
    pub game_id: GameId,
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub white: VerifyingKey,
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub black: VerifyingKey,
    pub white_nickname: String,
    pub black_nickname: String,
    pub result: GameResult,
    /// The complete move list — this is what makes replay work.
    pub moves: Vec<Move>,
    pub time_control: TimeControl,
    pub started_at: i64,
    pub finished_at: i64,
    /// Ratings the two players held going in, agreed by both signatures. The
    /// player contract replays these to recompute a rating without having to
    /// fetch every opponent's profile.
    pub white_rating_before: i32,
    pub black_rating_before: i32,
    #[serde(with = "crate::identity::signature_serde")]
    pub white_signature: Signature,
    #[serde(with = "crate::identity::signature_serde")]
    pub black_signature: Signature,
}

/// Everything both players sign, excluding the signatures themselves.
///
/// Long argument list on purpose: taking the fields individually is what lets
/// [`CertificateDraft`] and [`GameCertificate`] produce byte-identical signing
/// input without one having to construct the other.
#[allow(clippy::too_many_arguments)]
fn certificate_signing_bytes(
    game_id: &GameId,
    white: &VerifyingKey,
    black: &VerifyingKey,
    white_nickname: &str,
    black_nickname: &str,
    result: GameResult,
    moves: &[Move],
    time_control: &TimeControl,
    started_at: i64,
    finished_at: i64,
    white_rating_before: i32,
    black_rating_before: i32,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256 + moves.len() * 3);
    buf.extend_from_slice(SIG_DOMAIN);
    buf.extend_from_slice(b"certificate:");
    buf.extend_from_slice(&game_id.0);
    buf.extend_from_slice(white.as_bytes());
    buf.extend_from_slice(black.as_bytes());
    // Length-prefix every variable-length field so no two distinct
    // certificates can produce the same signing bytes.
    buf.extend_from_slice(&(white_nickname.len() as u32).to_le_bytes());
    buf.extend_from_slice(white_nickname.as_bytes());
    buf.extend_from_slice(&(black_nickname.len() as u32).to_le_bytes());
    buf.extend_from_slice(black_nickname.as_bytes());
    buf.push(result_tag(result));
    buf.extend_from_slice(&(moves.len() as u32).to_le_bytes());
    for mv in moves {
        buf.extend_from_slice(&mv.signing_bytes());
    }
    buf.extend_from_slice(&time_control.initial_secs.to_le_bytes());
    buf.extend_from_slice(&time_control.increment_secs.to_le_bytes());
    buf.extend_from_slice(&started_at.to_le_bytes());
    buf.extend_from_slice(&finished_at.to_le_bytes());
    buf.extend_from_slice(&white_rating_before.to_le_bytes());
    buf.extend_from_slice(&black_rating_before.to_le_bytes());
    buf
}

/// Stable one-byte encoding of a result, so the signed bytes never shift when
/// the enum gains variants. Shared with the lobby's snapshot signatures.
pub fn result_tag(result: GameResult) -> u8 {
    match result {
        GameResult::InProgress => 0,
        GameResult::AwaitingOpponent => 1,
        GameResult::WhiteWins(WinReason::Checkmate) => 10,
        GameResult::WhiteWins(WinReason::Resignation) => 11,
        GameResult::WhiteWins(WinReason::Timeout) => 12,
        GameResult::BlackWins(WinReason::Checkmate) => 20,
        GameResult::BlackWins(WinReason::Resignation) => 21,
        GameResult::BlackWins(WinReason::Timeout) => 22,
        GameResult::Draw(DrawReason::Stalemate) => 30,
        GameResult::Draw(DrawReason::Agreement) => 31,
        GameResult::Draw(DrawReason::InsufficientMaterial) => 32,
        GameResult::Draw(DrawReason::FiftyMoveRule) => 33,
        GameResult::Draw(DrawReason::ThreefoldRepetition) => 34,
    }
}

/// An unsigned certificate, ready for both players to sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateDraft {
    pub game_id: GameId,
    pub white: VerifyingKey,
    pub black: VerifyingKey,
    pub white_nickname: String,
    pub black_nickname: String,
    pub result: GameResult,
    pub moves: Vec<Move>,
    pub time_control: TimeControl,
    pub started_at: i64,
    pub finished_at: i64,
    pub white_rating_before: i32,
    pub black_rating_before: i32,
}

impl CertificateDraft {
    /// Build the draft both clients will sign from a finished game.
    ///
    /// Returns `None` if the game is not actually over — there is nothing to
    /// certify yet.
    pub fn from_game(
        state: &ChessGameStateV1,
        game_id: GameId,
        finished_at: i64,
        white_rating_before: i32,
        black_rating_before: i32,
    ) -> Option<CertificateDraft> {
        let result = state.result();
        if !result.is_over() {
            return None;
        }
        let (white, black) = state.player_keys()?;
        let setup = state.setup.get()?;
        let join = state.opponent.get()?;
        let (white_nickname, black_nickname) = match setup.setup.creator_plays {
            Color::White => (setup.setup.creator_nickname.clone(), join.nickname.clone()),
            Color::Black => (join.nickname.clone(), setup.setup.creator_nickname.clone()),
        };
        Some(CertificateDraft {
            game_id,
            white,
            black,
            white_nickname,
            black_nickname,
            result,
            moves: state.moves.move_list(),
            time_control: setup.setup.time_control,
            started_at: join.joined_at.max(setup.setup.created_at),
            finished_at,
            white_rating_before,
            black_rating_before,
        })
    }

    fn signing_bytes(&self) -> Vec<u8> {
        certificate_signing_bytes(
            &self.game_id,
            &self.white,
            &self.black,
            &self.white_nickname,
            &self.black_nickname,
            self.result,
            &self.moves,
            &self.time_control,
            self.started_at,
            self.finished_at,
            self.white_rating_before,
            self.black_rating_before,
        )
    }

    /// One player's signature over this draft. Each client signs
    /// independently; whoever holds both assembles the certificate.
    pub fn sign(&self, key: &SigningKey) -> Signature {
        key.sign(&self.signing_bytes())
    }

    /// Combine the two signatures into a finished certificate.
    pub fn assemble(
        self,
        white_signature: Signature,
        black_signature: Signature,
    ) -> GameCertificate {
        GameCertificate {
            game_id: self.game_id,
            white: self.white,
            black: self.black,
            white_nickname: self.white_nickname,
            black_nickname: self.black_nickname,
            result: self.result,
            moves: self.moves,
            time_control: self.time_control,
            started_at: self.started_at,
            finished_at: self.finished_at,
            white_rating_before: self.white_rating_before,
            black_rating_before: self.black_rating_before,
            white_signature,
            black_signature,
        }
    }
}

impl GameCertificate {
    fn signing_bytes(&self) -> Vec<u8> {
        certificate_signing_bytes(
            &self.game_id,
            &self.white,
            &self.black,
            &self.white_nickname,
            &self.black_nickname,
            self.result,
            &self.moves,
            &self.time_control,
            self.started_at,
            self.finished_at,
            self.white_rating_before,
            self.black_rating_before,
        )
    }

    /// Full validation from the bytes alone: both signatures, a legal move
    /// list, and a board result that matches the position it claims.
    pub fn verify(&self) -> Result<(), String> {
        if self.white == self.black {
            return Err("a certificate cannot have the same player on both sides".to_string());
        }
        if !self.result.is_over() {
            return Err("certificate records a game that is not over".to_string());
        }
        if self.moves.len() > MAX_CERTIFIED_PLIES {
            return Err(format!(
                "certificate carries {} plies, over the {MAX_CERTIFIED_PLIES} limit",
                self.moves.len()
            ));
        }
        // The game id is a proof-of-work digest, so requiring the work here is
        // what makes a certificate cost something. Without it the archive, every
        // profile history and the leaderboard are free to fill with fabricated
        // games: two throwaway keys are enough, and keys are free.
        //
        // This prices a *slot* rather than proving a real game happened — every
        // collection is keyed by game id, so N entries cost N mined ids, the same
        // as N honest games. It does not bind the id to either player's key,
        // which would need the certificate to carry the setup terms it is
        // derived from; two colluding keys can still co-sign a fabricated result
        // between themselves, which the module docs above already concede is
        // inherent.
        if leading_zero_bits(&self.game_id.0) < POW_DIFFICULTY_BITS {
            return Err(format!(
                "certificate game id carries {} bits of proof-of-work, need {}",
                leading_zero_bits(&self.game_id.0),
                POW_DIFFICULTY_BITS
            ));
        }
        if self.finished_at < self.started_at {
            return Err("certificate finishes before it starts".to_string());
        }
        if self.started_at <= 0 {
            return Err("certificate has a non-positive start time".to_string());
        }
        // Bounded because `finished_at` places a game in an archive shard and
        // orders it there, so an absurd value spawns shard contracts at absurd
        // epochs and evicts real games from the ones it lands in.
        if self.finished_at > MAX_CERTIFICATE_TIME_MS {
            return Err(format!(
                "certificate finishes at {}, past the {MAX_CERTIFICATE_TIME_MS} bound",
                self.finished_at
            ));
        }
        // Bounded everywhere else in the crate; this path was the exception.
        for (whose, nickname) in [
            ("white", &self.white_nickname),
            ("black", &self.black_nickname),
        ] {
            if nickname.len() > MAX_NICKNAME_LEN {
                return Err(format!(
                    "{whose}'s nickname is longer than {MAX_NICKNAME_LEN} bytes"
                ));
            }
        }
        // Bounded because these feed `elo`, whose arithmetic is only meaningful
        // over real ratings.
        for (whose, rating) in [
            ("white", self.white_rating_before),
            ("black", self.black_rating_before),
        ] {
            if !(MIN_ELO..=MAX_ELO).contains(&rating) {
                return Err(format!(
                    "{whose}'s rating of {rating} is outside {MIN_ELO}..={MAX_ELO}"
                ));
            }
        }
        self.time_control.verify()?;

        let bytes = self.signing_bytes();
        verify_sig(
            &self.white,
            &bytes,
            &self.white_signature,
            "white's certificate",
        )?;
        verify_sig(
            &self.black,
            &bytes,
            &self.black_signature,
            "black's certificate",
        )?;

        // Re-play the moves: this is what makes the record trustworthy rather
        // than merely agreed.
        let game = Game::from_moves(&self.moves).map_err(|(ply, e)| {
            format!("certificate contains an illegal move at ply {ply}: {e}")
        })?;

        // Where the board itself decides the result, the claim must match it.
        // Results decided off the board rest on the two signatures instead.
        let board_result = match game.status() {
            GameStatus::Checkmate {
                winner: Color::White,
            } => Some(GameResult::WhiteWins(WinReason::Checkmate)),
            GameStatus::Checkmate {
                winner: Color::Black,
            } => Some(GameResult::BlackWins(WinReason::Checkmate)),
            GameStatus::Stalemate => Some(GameResult::Draw(DrawReason::Stalemate)),
            GameStatus::InsufficientMaterial => {
                Some(GameResult::Draw(DrawReason::InsufficientMaterial))
            }
            GameStatus::FiftyMoveRule => Some(GameResult::Draw(DrawReason::FiftyMoveRule)),
            GameStatus::ThreefoldRepetition => {
                Some(GameResult::Draw(DrawReason::ThreefoldRepetition))
            }
            GameStatus::InProgress => None,
        };
        match (board_result, self.result) {
            // The position is decisive: the certificate must say the same.
            (Some(actual), claimed) if actual != claimed => Err(format!(
                "certificate claims {claimed:?} but the final position is {actual:?}"
            )),
            // The position is still playable, so only an off-board result is
            // coherent — a claimed checkmate would be a lie.
            (
                None,
                GameResult::WhiteWins(WinReason::Checkmate)
                | GameResult::BlackWins(WinReason::Checkmate)
                | GameResult::Draw(DrawReason::Stalemate)
                | GameResult::Draw(DrawReason::InsufficientMaterial)
                | GameResult::Draw(DrawReason::FiftyMoveRule)
                | GameResult::Draw(DrawReason::ThreefoldRepetition),
            ) => Err(
                "certificate claims a board result that the final position does not support"
                    .to_string(),
            ),
            _ => Ok(()),
        }
    }

    pub fn white_id(&self) -> PlayerId {
        PlayerId::from(&self.white)
    }

    pub fn black_id(&self) -> PlayerId {
        PlayerId::from(&self.black)
    }

    /// Does this certificate involve `player`?
    pub fn involves(&self, player: PlayerId) -> bool {
        self.white_id() == player || self.black_id() == player
    }

    /// Which color `player` had, if they played in this game.
    pub fn color_of(&self, player: PlayerId) -> Option<Color> {
        if self.white_id() == player {
            Some(Color::White)
        } else if self.black_id() == player {
            Some(Color::Black)
        } else {
            None
        }
    }

    /// This game's score from `player`'s point of view.
    pub fn score_for(&self, player: PlayerId) -> Option<crate::elo::Score> {
        let color = self.color_of(player)?;
        let white_score = self.result.score_for_white()?;
        Some(match color {
            Color::White => white_score,
            Color::Black => white_score.inverse(),
        })
    }

    /// The opponent's rating going in, from `player`'s point of view.
    pub fn opponent_rating_for(&self, player: PlayerId) -> Option<i32> {
        Some(match self.color_of(player)? {
            Color::White => self.black_rating_before,
            Color::Black => self.white_rating_before,
        })
    }

    /// Replay the certified game, for the board and the move list.
    pub fn replay(&self) -> Game {
        Game::from_moves(&self.moves).unwrap_or_else(|_| Game::new())
    }

    /// Export as PGN, the portable format every chess tool reads.
    pub fn to_pgn(&self) -> String {
        let date = format_date(self.started_at);
        self.replay().to_pgn(
            &[
                ("Event", "FreeChess".to_string()),
                ("Site", "Freenet".to_string()),
                ("Date", date),
                ("White", self.white_nickname.clone()),
                ("Black", self.black_nickname.clone()),
                ("WhiteElo", self.white_rating_before.to_string()),
                ("BlackElo", self.black_rating_before.to_string()),
                ("TimeControl", self.time_control.label()),
            ],
            self.result.pgn_token(),
        )
    }

    /// Deterministic ordering key for archives and history: newest first is
    /// applied by the caller, ties broken by game id so every peer agrees.
    pub fn order_key(&self) -> (i64, [u8; 32]) {
        (self.finished_at, self.game_id.0)
    }
}

/// `YYYY.MM.DD` in UTC, the PGN date format.
fn format_date(unix_ms: i64) -> String {
    use chrono::TimeZone;
    match chrono::Utc.timestamp_millis_opt(unix_ms).single() {
        Some(dt) => dt.format("%Y.%m.%d").to_string(),
        None => "????.??.??".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::setup::{leading_zero_bits, MAX_NICKNAME_LEN, POW_DIFFICULTY_BITS};
    use crate::testutil::{certified_game, key, T0};

    /// A certificate over `game_id` that both throwaway keys signed. Nothing
    /// about it corresponds to a real game.
    fn forged(game_id: GameId, mutate: impl FnOnce(&mut CertificateDraft)) -> GameCertificate {
        let white = key();
        let black = key();
        let mut draft = CertificateDraft {
            game_id,
            white: white.verifying_key(),
            black: black.verifying_key(),
            white_nickname: "w".to_string(),
            black_nickname: "b".to_string(),
            result: GameResult::WhiteWins(WinReason::Resignation),
            moves: Vec::new(),
            time_control: TimeControl::default(),
            started_at: T0,
            finished_at: T0 + 1_000,
            white_rating_before: 1200,
            black_rating_before: 1200,
        };
        mutate(&mut draft);
        let ws = draft.sign(&white);
        let bs = draft.sign(&black);
        draft.assemble(ws, bs)
    }

    #[test]
    fn a_certificate_costs_proof_of_work() {
        // Without this the archive, every profile history and the leaderboard
        // are free to spam: two throwaway keys co-sign a fabricated result and
        // it verifies. The work is what prices a slot, exactly as it does for
        // creating a game.
        let cert = forged(GameId([0xff; 32]), |_| {});
        let err = cert.verify().expect_err("must reject");
        assert!(err.contains("proof-of-work"), "got: {err}");

        // A real game's id carries the work, so a genuine certificate passes.
        let (real, game_id) = certified_game(&key(), &key(), T0, 1200, 1200);
        assert!(leading_zero_bits(&game_id.0) >= POW_DIFFICULTY_BITS);
        real.verify().expect("a real game's certificate verifies");
    }

    #[test]
    fn unbounded_certificate_fields_are_rejected() {
        // Every one of these reaches code that assumes a sane value: nicknames
        // are bounded everywhere else, and the ratings feed `elo`.
        let (real, game_id) = certified_game(&key(), &key(), T0, 1200, 1200);
        let _ = real;

        let long_name = forged(game_id, |d| {
            d.white_nickname = "n".repeat(MAX_NICKNAME_LEN + 1);
        });
        assert!(
            long_name.verify().unwrap_err().contains("nickname"),
            "an over-long nickname must be rejected"
        );

        let absurd_rating = forged(game_id, |d| {
            d.white_rating_before = i32::MIN;
        });
        assert!(
            absurd_rating.verify().unwrap_err().contains("rating"),
            "a rating outside the Elo range must be rejected"
        );

        let far_future = forged(game_id, |d| {
            d.finished_at = i64::MAX;
        });
        assert!(
            far_future.verify().is_err(),
            "an absurd finish time must be rejected"
        );
    }
}
