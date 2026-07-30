//! The lobby: a single, well-known contract listing every live game.
//!
//! # One subscription shows the whole site
//!
//! Each entry carries a signed snapshot of its game's current position (FEN,
//! ply, clocks). A client that subscribes to *this one contract* therefore
//! receives live board updates for every game in progress — which is what makes
//! the home page work without opening a subscription per game. Opening a
//! specific game additionally subscribes to that game's own contract, for the
//! full move list and interactive play.
//!
//! # Anti-spam
//!
//! Three independent limits, all verifiable from state alone:
//!
//! 1. **Proof-of-work per game.** Every listing embeds the creator-signed setup
//!    whose `game_id` must be a PoW digest (see
//!    [`crate::game::setup`]). Minting a listing costs work, so a fresh keypair
//!    buys nothing.
//! 2. **A quota per creator.** At most [`MAX_OPEN_PER_CREATOR`] games *waiting
//!    for an opponent* and [`MAX_LISTINGS_PER_CREATOR`] listings in total.
//!    Filling the lobby with unanswered challenges is therefore impossible
//!    regardless of how much work an attacker burns.
//! 3. **A global cap.** [`MAX_LOBBY_ENTRIES`] listings overall.
//!
//! Every one of these is enforced by *deterministic eviction over the merged
//! set* — sorted by a total order and truncated — never by arrival order. That
//! is what keeps the state an idempotent commutative monoid, as the whitepaper
//! requires; an order-sensitive cap would silently diverge peers.

use crate::chess::Color;
use crate::game::opponent::SignedJoin;
use crate::game::setup::{SignedGameSetup, TimeControl, MAX_NICKNAME_LEN};
use crate::game::{GameResult, SIG_DOMAIN};
use crate::identity::{verify_sig, GameId, PlayerId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold_macro::composable;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// Games one creator may have open (waiting for a challenger) at once.
pub const MAX_OPEN_PER_CREATOR: usize = 3;

/// Total listings one creator may occupy, open or running.
pub const MAX_LISTINGS_PER_CREATOR: usize = 20;

/// Listings the lobby holds in total.
pub const MAX_LOBBY_ENTRIES: usize = 500;

/// Longest FEN string accepted in a snapshot; a valid FEN is comfortably under
/// this, and the bound stops a peer inflating state with a huge string.
const MAX_FEN_LEN: usize = 100;

/// The lobby is a singleton, so its parameters carry only a namespace constant
/// rather than an owner: there is no privileged writer, and every entry
/// authorizes itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyParametersV1 {
    /// Fixed namespace tag. Distinct values yield distinct lobbies, which is
    /// how a test network stays separate from the public one.
    pub namespace: String,
}

impl Default for LobbyParametersV1 {
    fn default() -> Self {
        LobbyParametersV1 {
            namespace: "freechess:lobby:v1".to_string(),
        }
    }
}

/// A player's live view of a game's position, signed by one of the two players.
///
/// This is what the home page renders. It is a convenience mirror of the game
/// contract, not the source of truth — a stale or missing snapshot costs
/// nothing, because opening the game reads the real state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedSnapshot {
    pub fen: String,
    /// Plies played, used to keep the newest snapshot when two race.
    pub ply: u32,
    pub result: GameResult,
    /// Milliseconds left for each side at `updated_at`.
    pub white_ms: i64,
    pub black_ms: i64,
    pub updated_at: i64,
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub author: VerifyingKey,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl SignedSnapshot {
    #[allow(clippy::too_many_arguments)]
    fn signing_bytes(
        game_id: &GameId,
        fen: &str,
        ply: u32,
        result: GameResult,
        white_ms: i64,
        black_ms: i64,
        updated_at: i64,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(160);
        buf.extend_from_slice(SIG_DOMAIN);
        buf.extend_from_slice(b"snapshot:");
        buf.extend_from_slice(&game_id.0);
        buf.extend_from_slice(&(fen.len() as u32).to_le_bytes());
        buf.extend_from_slice(fen.as_bytes());
        buf.extend_from_slice(&ply.to_le_bytes());
        buf.push(crate::certificate::result_tag(result));
        buf.extend_from_slice(&white_ms.to_le_bytes());
        buf.extend_from_slice(&black_ms.to_le_bytes());
        buf.extend_from_slice(&updated_at.to_le_bytes());
        buf
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key: &SigningKey,
        game_id: &GameId,
        fen: String,
        ply: u32,
        result: GameResult,
        white_ms: i64,
        black_ms: i64,
        updated_at: i64,
    ) -> SignedSnapshot {
        let signature = key.sign(&Self::signing_bytes(
            game_id, &fen, ply, result, white_ms, black_ms, updated_at,
        ));
        SignedSnapshot {
            fen,
            ply,
            result,
            white_ms,
            black_ms,
            updated_at,
            author: key.verifying_key(),
            signature,
        }
    }

    /// Valid if signed by one of the two keys allowed to speak for this game.
    fn verify(&self, game_id: &GameId, players: &[VerifyingKey]) -> Result<(), String> {
        if self.fen.len() > MAX_FEN_LEN {
            return Err("snapshot FEN is implausibly long".to_string());
        }
        if !players.contains(&self.author) {
            return Err("snapshot signed by someone who is not a player in this game".to_string());
        }
        verify_sig(
            &self.author,
            &Self::signing_bytes(
                game_id,
                &self.fen,
                self.ply,
                self.result,
                self.white_ms,
                self.black_ms,
                self.updated_at,
            ),
            &self.signature,
            "snapshot",
        )
    }

    /// Newest snapshot wins; ties broken by signature bytes for determinism.
    fn order_key(&self) -> (u32, i64, [u8; 64]) {
        (self.ply, self.updated_at, self.signature.to_bytes())
    }
}

/// One game as the lobby sees it.
///
/// Every component is independently self-authorizing, so the lobby needs no
/// privileged writer: `setup` is signed by the creator and bound to the
/// proof-of-work `game_id`, `opponent` is signed by the challenger over the
/// same `game_id`, and `snapshot` must be signed by one of those two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyEntry {
    pub game_id: GameId,
    pub setup: SignedGameSetup,
    #[serde(default)]
    pub opponent: Option<SignedJoin>,
    #[serde(default)]
    pub snapshot: Option<SignedSnapshot>,
}

impl LobbyEntry {
    pub fn new(game_id: GameId, setup: SignedGameSetup) -> LobbyEntry {
        LobbyEntry {
            game_id,
            setup,
            opponent: None,
            snapshot: None,
        }
    }

    pub fn creator(&self) -> VerifyingKey {
        self.setup.creator
    }

    pub fn creator_id(&self) -> PlayerId {
        PlayerId::from(&self.setup.creator)
    }

    pub fn created_at(&self) -> i64 {
        self.setup.setup.created_at
    }

    pub fn time_control(&self) -> TimeControl {
        self.setup.setup.time_control
    }

    /// Still waiting for a challenger.
    pub fn is_open(&self) -> bool {
        self.opponent.is_none()
    }

    /// Being played right now — what the home page shows as "live".
    pub fn is_live(&self) -> bool {
        self.opponent.is_some() && !self.result().is_over()
    }

    pub fn result(&self) -> GameResult {
        match &self.snapshot {
            Some(s) => s.result,
            None if self.opponent.is_some() => GameResult::InProgress,
            None => GameResult::AwaitingOpponent,
        }
    }

    /// The two keys entitled to speak for this game, as far as the lobby knows.
    fn player_keys(&self) -> Vec<VerifyingKey> {
        let mut keys = vec![self.setup.creator];
        if let Some(join) = &self.opponent {
            keys.push(join.player);
        }
        keys
    }

    /// Nicknames as `(white, black)`.
    pub fn nicknames(&self) -> (String, String) {
        let creator = self.setup.setup.creator_nickname.clone();
        let challenger = self
            .opponent
            .as_ref()
            .map(|j| j.nickname.clone())
            .unwrap_or_else(|| "—".to_string());
        match self.setup.setup.creator_plays {
            Color::White => (creator, challenger),
            Color::Black => (challenger, creator),
        }
    }

    /// Position to render, falling back to the initial position before any
    /// snapshot has been published.
    pub fn fen(&self) -> String {
        self.snapshot
            .as_ref()
            .map(|s| s.fen.clone())
            .unwrap_or_else(|| crate::chess::STARTING_FEN.to_string())
    }

    /// Most recent activity, used to order the lobby.
    pub fn last_activity(&self) -> i64 {
        self.snapshot
            .as_ref()
            .map(|s| s.updated_at)
            .or_else(|| self.opponent.as_ref().map(|j| j.joined_at))
            .unwrap_or_else(|| self.created_at())
    }

    /// Check every signature and the proof-of-work binding.
    pub fn verify(&self) -> Result<(), String> {
        // Rebuild the parameters this entry claims and let the game's own
        // verifier check the PoW and the creator signature — the exact same
        // code the game contract runs.
        let params = crate::game::ChessGameParametersV1 {
            creator: self.setup.creator,
            game_id: self.game_id,
        };
        self.setup.verify(&params)?;
        if let Some(join) = &self.opponent {
            join.verify(&params)?;
            // Same rule the game contract applies: a direct challenge may only
            // be accepted by the player it names.
            if let Some(invited) = self.setup.setup.challenged {
                if join.player != invited {
                    return Err("this game is a direct challenge to a different player".to_string());
                }
            }
        }
        if let Some(snapshot) = &self.snapshot {
            snapshot.verify(&self.game_id, &self.player_keys())?;
        }
        if self.setup.setup.creator_nickname.len() > MAX_NICKNAME_LEN {
            return Err("creator nickname too long".to_string());
        }
        Ok(())
    }

    /// Merge another view of the same game into this one. Each component takes
    /// the winner of a total order, so the result is independent of order and
    /// stable under repetition.
    fn absorb(&mut self, other: LobbyEntry) {
        if self.opponent.is_none() {
            self.opponent = other.opponent;
        } else if let (Some(mine), Some(theirs)) = (&self.opponent, &other.opponent) {
            // Same deterministic race rule the game contract uses, so the lobby
            // never disagrees with the game about who is playing.
            if (theirs.joined_at, theirs.player.to_bytes())
                < (mine.joined_at, mine.player.to_bytes())
            {
                self.opponent = other.opponent;
            }
        }

        match (&self.snapshot, &other.snapshot) {
            (None, Some(_)) => self.snapshot = other.snapshot,
            (Some(mine), Some(theirs)) if theirs.order_key() > mine.order_key() => {
                self.snapshot = other.snapshot
            }
            _ => {}
        }
    }
}

/// The lobby's game index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyGamesV1 {
    /// Keyed by game id, so the summary is deterministic.
    pub games: BTreeMap<GameId, LobbyEntry>,
}

impl LobbyGamesV1 {
    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    pub fn get(&self, id: &GameId) -> Option<&LobbyEntry> {
        self.games.get(id)
    }

    fn absorb(&mut self, incoming: LobbyEntry) {
        match self.games.get_mut(&incoming.game_id) {
            Some(existing) => existing.absorb(incoming),
            None => {
                self.games.insert(incoming.game_id, incoming);
            }
        }
    }

    /// Games that have finished, most recently active first.
    ///
    /// A game leaves `live_games` and appears here as soon as its snapshot
    /// carries a decided result, so the home page can move it out of "in
    /// progress" without waiting for anything else.
    pub fn finished_games(&self) -> Vec<&LobbyEntry> {
        let mut out: Vec<&LobbyEntry> = self
            .games
            .values()
            .filter(|e| e.result().is_over())
            .collect();
        out.sort_by_key(|e| (std::cmp::Reverse(e.last_activity()), e.game_id.0));
        out
    }

    /// Games waiting for a challenger, newest first.
    pub fn open_games(&self) -> Vec<&LobbyEntry> {
        let mut out: Vec<&LobbyEntry> = self.games.values().filter(|e| e.is_open()).collect();
        out.sort_by_key(|e| (std::cmp::Reverse(e.created_at()), e.game_id.0));
        out
    }

    /// Games being played right now, most recently active first. This is the
    /// home page's main list.
    pub fn live_games(&self) -> Vec<&LobbyEntry> {
        let mut out: Vec<&LobbyEntry> = self.games.values().filter(|e| e.is_live()).collect();
        out.sort_by_key(|e| (std::cmp::Reverse(e.last_activity()), e.game_id.0));
        out
    }

    /// Open games anyone may join — direct challenges are excluded, since only
    /// their invitee can accept.
    pub fn public_open_games(&self) -> Vec<&LobbyEntry> {
        self.open_games()
            .into_iter()
            .filter(|e| e.setup.setup.challenged.is_none())
            .collect()
    }

    /// Pending challenges addressed to `player`, newest first. This is the
    /// player's "you have been invited" list.
    pub fn challenges_for(&self, player: PlayerId) -> Vec<&LobbyEntry> {
        self.open_games()
            .into_iter()
            .filter(|e| {
                e.setup
                    .setup
                    .challenged
                    .map(|k| PlayerId::from(&k) == player)
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Free-text search over nicknames, plus exact game-id lookup.
    pub fn search(&self, query: &str) -> Vec<&LobbyEntry> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<&LobbyEntry> = self
            .games
            .values()
            .filter(|e| {
                let (white, black) = e.nicknames();
                white.to_lowercase().contains(&needle)
                    || black.to_lowercase().contains(&needle)
                    || e.game_id.to_base58().to_lowercase().starts_with(&needle)
            })
            .collect();
        out.sort_by_key(|e| (std::cmp::Reverse(e.last_activity()), e.game_id.0));
        out
    }

    /// Apply every quota by deterministic eviction over the merged set.
    ///
    /// Pure and idempotent: sorting by a total order and truncating gives the
    /// same survivors on every peer, no matter what order entries arrived in or
    /// how many times this runs.
    pub fn enforce_quotas(&mut self) {
        // Per-creator caps. Newest first, so a creator who hits the cap
        // replaces their own stalest listing rather than being locked out.
        let mut by_creator: BTreeMap<PlayerId, Vec<GameId>> = BTreeMap::new();
        for entry in self.games.values() {
            by_creator
                .entry(entry.creator_id())
                .or_default()
                .push(entry.game_id);
        }

        let mut doomed: Vec<GameId> = Vec::new();
        for (_creator, mut ids) in by_creator {
            ids.sort_by_key(|id| {
                let e = &self.games[id];
                (std::cmp::Reverse(e.created_at()), *id)
            });

            let mut open_seen = 0usize;
            for (rank, id) in ids.iter().enumerate() {
                let entry = &self.games[id];
                if rank >= MAX_LISTINGS_PER_CREATOR {
                    doomed.push(*id);
                    continue;
                }
                if entry.is_open() {
                    open_seen += 1;
                    if open_seen > MAX_OPEN_PER_CREATOR {
                        doomed.push(*id);
                    }
                }
            }
        }
        for id in doomed {
            self.games.remove(&id);
        }

        // Global cap: keep the most recently active games.
        if self.games.len() > MAX_LOBBY_ENTRIES {
            let mut ids: Vec<GameId> = self.games.keys().copied().collect();
            ids.sort_by_key(|id| {
                let e = &self.games[id];
                (std::cmp::Reverse(e.last_activity()), *id)
            });
            for id in ids.into_iter().skip(MAX_LOBBY_ENTRIES) {
                self.games.remove(&id);
            }
        }
    }

    /// How many listings a creator currently holds, and how many are open.
    pub fn creator_counts(&self, creator: PlayerId) -> (usize, usize) {
        let mut total = 0;
        let mut open = 0;
        for entry in self.games.values() {
            if entry.creator_id() == creator {
                total += 1;
                if entry.is_open() {
                    open += 1;
                }
            }
        }
        (total, open)
    }
}

impl ComposableState for LobbyGamesV1 {
    type ParentState = LobbyStateV1;
    type Summary = Vec<(GameId, u32)>;
    type Delta = Vec<LobbyEntry>;
    type Parameters = LobbyParametersV1;

    fn verify(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
    ) -> Result<(), String> {
        if self.games.len() > MAX_LOBBY_ENTRIES {
            return Err(format!(
                "lobby holds {} entries, over the {MAX_LOBBY_ENTRIES} cap",
                self.games.len()
            ));
        }
        for (id, entry) in &self.games {
            if &entry.game_id != id {
                return Err("lobby entry stored under the wrong game id".to_string());
            }
            entry.verify()?;
        }

        // Re-check the per-creator quotas so a full-state PUT (which bypasses
        // the merge path, and therefore `enforce_quotas`) cannot smuggle in a
        // flooded lobby.
        let mut totals: BTreeMap<PlayerId, (usize, usize)> = BTreeMap::new();
        for entry in self.games.values() {
            let counts = totals.entry(entry.creator_id()).or_insert((0, 0));
            counts.0 += 1;
            if entry.is_open() {
                counts.1 += 1;
            }
        }
        for (creator, (total, open)) in totals {
            if total > MAX_LISTINGS_PER_CREATOR {
                return Err(format!(
                    "creator {creator} holds {total} listings, over the {MAX_LISTINGS_PER_CREATOR} cap"
                ));
            }
            if open > MAX_OPEN_PER_CREATOR {
                return Err(format!(
                    "creator {creator} holds {open} open games, over the {MAX_OPEN_PER_CREATOR} cap"
                ));
            }
        }
        Ok(())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        // (id, revision) where revision advances as a game progresses, so a
        // peer can tell "same game, newer position" without shipping state.
        self.games
            .iter()
            .map(|(id, e)| {
                let rev = e.snapshot.as_ref().map(|s| s.ply + 1).unwrap_or(0)
                    + u32::from(e.opponent.is_some());
                (*id, rev)
            })
            .collect()
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let theirs: BTreeMap<GameId, u32> = old_summary.iter().copied().collect();
        let changed: Vec<LobbyEntry> = self
            .games
            .iter()
            .filter(|(id, e)| {
                let rev = e.snapshot.as_ref().map(|s| s.ply + 1).unwrap_or(0)
                    + u32::from(e.opponent.is_some());
                theirs
                    .get(id)
                    .map(|their_rev| *their_rev < rev)
                    .unwrap_or(true)
            })
            .map(|(_, e)| e.clone())
            .collect();
        if changed.is_empty() {
            None
        } else {
            Some(changed)
        }
    }

    fn apply_delta(
        &mut self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(entries) = delta {
            for entry in entries {
                // Reject forgeries outright; quotas are applied afterwards over
                // the whole merged set.
                entry.verify()?;
                self.absorb(entry.clone());
            }
        }
        Ok(())
    }
}

/// The lobby contract's state: the live game index and the Elo ranking.
///
/// Both live in one contract on purpose. The home page needs the running games
/// *and* the ranking, so bundling them means a client opens a **single
/// subscription** and receives every live board update and every rating change
/// from it.
#[composable(post_apply_delta = "prune")]
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
pub struct LobbyStateV1 {
    pub games: LobbyGamesV1,
    pub leaderboard: crate::leaderboard::LeaderboardV1,
    /// Who is online, away or offline. Rides here so the home page's single
    /// subscription also keeps the player list current.
    pub presence: crate::presence::PresenceV1,
    /// Admins, announcements, game takedowns and the service notice.
    pub administration: crate::admin::AdministrationV1,
}

impl LobbyStateV1 {
    /// The current display name for a player, looked up by id.
    ///
    /// Names embedded in a game's setup, join or certificate are frozen: they
    /// are covered by signatures made when the game started, so they cannot be
    /// rewritten later. Presence, by contrast, is republished on every
    /// heartbeat and whenever the player renames. Resolving by id here is what
    /// lets the UI show the *current* name everywhere while the signed records
    /// stay verifiable — with the embedded copy as a fallback for a player who
    /// is not currently around.
    pub fn display_name(&self, player: PlayerId, embedded: &str) -> String {
        self.presence
            .players
            .get(&player)
            .map(|p| p.nickname.clone())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| embedded.to_string())
    }

    /// `(white, black)` display names for a game, both resolved by id.
    pub fn game_names(&self, entry: &LobbyEntry) -> (String, String) {
        let (white_embedded, black_embedded) = entry.nicknames();
        let creator = entry.creator_id();
        let challenger = entry.opponent.as_ref().map(|j| j.player_id());

        match entry.setup.setup.creator_plays {
            Color::White => (
                self.display_name(creator, &white_embedded),
                challenger
                    .map(|id| self.display_name(id, &black_embedded))
                    .unwrap_or(black_embedded),
            ),
            Color::Black => (
                challenger
                    .map(|id| self.display_name(id, &white_embedded))
                    .unwrap_or(white_embedded),
                self.display_name(creator, &black_embedded),
            ),
        }
    }

    /// Apply the anti-spam quotas and the ranking/presence caps after every
    /// merge.
    pub fn prune(&mut self, _params: &LobbyParametersV1) -> Result<(), String> {
        self.games.enforce_quotas();
        self.leaderboard.prune();
        self.presence.prune();
        self.administration.prune();
        Ok(())
    }
}
