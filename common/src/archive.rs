//! The shared game history — every finished game, replayable by anyone.
//!
//! # Sharding
//!
//! One contract cannot hold the whole history, so the archive is split into
//! shards of at most [`GAMES_PER_SHARD`] games. A game's shard is *derived*
//! from the game itself:
//!
//! ```text
//! shard = (day = finished_at / 24h,  bucket = first byte of game_id % BUCKETS)
//! ```
//!
//! Nothing coordinates this. Any peer computes where a game belongs, and where
//! to look for it, from data it already has — no index contract, no allocator,
//! no writer that could become a bottleneck. Reading a day of history is
//! [`BUCKETS`] parallel GETs.
//!
//! # Why the history is needed even though games have their own contracts
//!
//! A game's own contract keeps working forever and is the authoritative record.
//! The archive exists so history is *discoverable*: browsing finished games,
//! searching by player, and pulling up a replay without already knowing the
//! game id. It stores co-signed [`GameCertificate`]s, which carry the full move
//! list, so a replay served from the archive is complete.

use crate::certificate::GameCertificate;
use crate::identity::{signature_digest, GameId, PlayerId};
use freenet_scaffold_macro::composable;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// Games one archive shard holds. Beyond this the oldest are evicted.
pub const GAMES_PER_SHARD: usize = 1000;

/// Buckets a day is split across. With [`GAMES_PER_SHARD`] this gives a
/// capacity of 16,000 games per day; raising it is a WASM change and therefore
/// needs a migration entry.
pub const BUCKETS: u8 = 16;

/// Length of an archive epoch.
pub const SHARD_DURATION_MS: i64 = 24 * 60 * 60 * 1000;

/// Which shard a game belongs in, derived purely from the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ShardId {
    /// Days since the Unix epoch, from the game's finish time.
    pub day: i64,
    pub bucket: u8,
}

impl ShardId {
    /// The shard a game finishing at `finished_at` with id `game_id` lands in.
    pub fn for_game(finished_at: i64, game_id: &GameId) -> ShardId {
        ShardId {
            day: finished_at.div_euclid(SHARD_DURATION_MS),
            bucket: game_id.0[0] % BUCKETS,
        }
    }

    /// Every bucket for a given day — what a client fetches to read one day of
    /// history.
    pub fn buckets_for_day(day: i64) -> Vec<ShardId> {
        (0..BUCKETS).map(|bucket| ShardId { day, bucket }).collect()
    }

    /// The day containing `unix_ms`.
    pub fn day_of(unix_ms: i64) -> i64 {
        unix_ms.div_euclid(SHARD_DURATION_MS)
    }
}

/// Parameters of one archive shard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveParametersV1 {
    pub namespace: String,
    pub shard: ShardId,
}

impl ArchiveParametersV1 {
    pub fn new(shard: ShardId) -> ArchiveParametersV1 {
        ArchiveParametersV1 {
            namespace: "freechess:archive:v1".to_string(),
            shard,
        }
    }
}

/// The certificates held by one shard.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedGamesV1 {
    pub games: BTreeMap<GameId, GameCertificate>,
}

impl ArchivedGamesV1 {
    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    pub fn get(&self, id: &GameId) -> Option<&GameCertificate> {
        self.games.get(id)
    }

    /// Certificates newest first.
    pub fn recent(&self) -> Vec<&GameCertificate> {
        let mut out: Vec<&GameCertificate> = self.games.values().collect();
        out.sort_by_key(|c| (std::cmp::Reverse(c.finished_at), c.game_id.0));
        out
    }

    /// Every certified game a player took part in, newest first.
    pub fn games_of(&self, player: PlayerId) -> Vec<&GameCertificate> {
        let mut out: Vec<&GameCertificate> =
            self.games.values().filter(|c| c.involves(player)).collect();
        out.sort_by_key(|c| (std::cmp::Reverse(c.finished_at), c.game_id.0));
        out
    }

    /// Search by nickname substring or game-id prefix.
    pub fn search(&self, query: &str) -> Vec<&GameCertificate> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<&GameCertificate> = self
            .games
            .values()
            .filter(|c| {
                c.white_nickname.to_lowercase().contains(&needle)
                    || c.black_nickname.to_lowercase().contains(&needle)
                    || c.game_id.to_base58().to_lowercase().starts_with(&needle)
            })
            .collect();
        out.sort_by_key(|c| (std::cmp::Reverse(c.finished_at), c.game_id.0));
        out
    }

    /// Insert if the certificate belongs here and is valid. Idempotent: a
    /// game id can only be written once, and re-inserting the same certificate
    /// changes nothing.
    fn absorb(&mut self, cert: GameCertificate) {
        match self.games.get(&cert.game_id) {
            None => {
                self.games.insert(cert.game_id, cert);
            }
            Some(existing) => {
                // Both are validly co-signed by the same two players for the
                // same game, so this only arises if the pair equivocated.
                // Settle it by a total order rather than by arrival.
                if cert.white_signature.to_bytes() < existing.white_signature.to_bytes() {
                    self.games.insert(cert.game_id, cert);
                }
            }
        }
    }

    /// Enforce the shard cap by deterministic eviction: keep the newest
    /// [`GAMES_PER_SHARD`] by `(finished_at, game_id)`.
    ///
    /// A pure function of the merged set, so every peer keeps the same games
    /// regardless of arrival order or how often this runs.
    fn enforce_cap(&mut self) {
        if self.games.len() <= GAMES_PER_SHARD {
            return;
        }
        let mut ids: Vec<GameId> = self.games.keys().copied().collect();
        ids.sort_by_key(|id| {
            let c = &self.games[id];
            (std::cmp::Reverse(c.finished_at), *id)
        });
        for id in ids.into_iter().skip(GAMES_PER_SHARD) {
            self.games.remove(&id);
        }
    }
}

impl ComposableState for ArchivedGamesV1 {
    type ParentState = ArchiveStateV1;
    /// Game id *and* a fingerprint of the certificate held for it. A bare set of
    /// ids meant a rival certificate for a game the peer already had never
    /// shipped, so the equivocation tiebreak never ran and the two archives
    /// stayed split.
    type Summary = Vec<(GameId, u64)>;
    type Delta = Vec<GameCertificate>;
    type Parameters = ArchiveParametersV1;

    fn verify(&self, _parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        if self.games.len() > GAMES_PER_SHARD {
            return Err(format!(
                "shard holds {} games, over the {GAMES_PER_SHARD} cap",
                self.games.len()
            ));
        }
        for (id, cert) in &self.games {
            if &cert.game_id != id {
                return Err("certificate stored under the wrong game id".to_string());
            }
            cert.verify()?;
            // A game must live in the shard its own data implies, otherwise one
            // shard could be stuffed with the whole history.
            let expected = ShardId::for_game(cert.finished_at, &cert.game_id);
            if expected != params.shard {
                return Err(format!(
                    "game {id} belongs in shard {expected:?}, not {:?}",
                    params.shard
                ));
            }
        }
        Ok(())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        self.games
            .iter()
            .map(|(id, c)| (*id, signature_digest(&c.white_signature)))
            .collect()
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let theirs: std::collections::BTreeMap<GameId, u64> = old_summary.iter().copied().collect();
        let missing: Vec<GameCertificate> = self
            .games
            .iter()
            .filter(|(id, c)| theirs.get(id) != Some(&signature_digest(&c.white_signature)))
            .map(|(_, c)| c.clone())
            .collect();
        if missing.is_empty() {
            None
        } else {
            Some(missing)
        }
    }

    fn apply_delta(
        &mut self,
        _parent: &Self::ParentState,
        params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(certs) = delta {
            for cert in certs {
                cert.verify()?;
                let expected = ShardId::for_game(cert.finished_at, &cert.game_id);
                if expected != params.shard {
                    return Err(format!(
                        "certificate for {} does not belong in this shard",
                        cert.game_id
                    ));
                }
                self.absorb(cert.clone());
            }
        }
        Ok(())
    }
}

/// One shard of the shared history.
#[composable(post_apply_delta = "prune")]
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
pub struct ArchiveStateV1 {
    pub games: ArchivedGamesV1,
}

impl ArchiveStateV1 {
    pub fn prune(&mut self, _params: &ArchiveParametersV1) -> Result<(), String> {
        self.games.enforce_cap();
        Ok(())
    }
}
