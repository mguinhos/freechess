//! Player presence: online, away, or offline.
//!
//! # Going offline without writing anything
//!
//! Presence is a signed heartbeat carrying a timestamp. A player is *online* if
//! their latest heartbeat is recent, *away* if it is older than
//! [`ONLINE_WINDOW_MS`] (the client also sets this explicitly when its tab
//! loses focus), and *offline* once it passes [`AWAY_WINDOW_MS`].
//!
//! Deriving offline from expiry rather than from a "goodbye" message matters
//! here: a peer that crashes, loses its connection, or closes the tab cannot
//! publish anything, so any design needing a final write would leave ghosts
//! online forever. It also keeps the merge trivially convergent — a heartbeat
//! is last-writer-wins on a timestamp, and time passing needs no update at all.
//!
//! Presence rides in the lobby contract, so the same single subscription that
//! feeds the home page its live games and ranking also keeps the player list
//! current.

use crate::game::setup::MAX_NICKNAME_LEN;
use crate::game::SIG_DOMAIN;
use crate::identity::{verify_sig, PlayerId};
use crate::lobby::{LobbyParametersV1, LobbyStateV1};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A heartbeat newer than this means the player is online.
pub const ONLINE_WINDOW_MS: i64 = 90_000;

/// Past this, the player is treated as offline.
pub const AWAY_WINDOW_MS: i64 = 10 * 60_000;

/// How often a client should re-publish its heartbeat. Comfortably inside
/// [`ONLINE_WINDOW_MS`] so one dropped update does not flicker the status.
pub const HEARTBEAT_INTERVAL_MS: i64 = 30_000;

/// Players tracked at once, capped to bound state.
pub const MAX_PRESENCE_ENTRIES: usize = 500;

/// What a player says they are doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresenceStatus {
    Online,
    /// Explicitly set by the client when the tab is backgrounded.
    Away,
    Offline,
}

impl PresenceStatus {
    /// CSS-friendly label used by the UI dot.
    pub fn label(self) -> &'static str {
        match self {
            PresenceStatus::Online => "online",
            PresenceStatus::Away => "away",
            PresenceStatus::Offline => "offline",
        }
    }
}

/// A signed heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedPresence {
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub player: VerifyingKey,
    pub nickname: String,
    /// What the player claims; the effective status also depends on age.
    pub claimed: PresenceStatus,
    /// The game this player is currently watching, if any. Lets the UI show a
    /// game's spectator list, so one spectator can pick another and challenge
    /// them directly.
    #[serde(default)]
    pub watching: Option<crate::identity::GameId>,
    pub updated_at: i64,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl SignedPresence {
    fn signing_bytes(
        player: &VerifyingKey,
        nickname: &str,
        claimed: PresenceStatus,
        watching: Option<crate::identity::GameId>,
        updated_at: i64,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(SIG_DOMAIN);
        buf.extend_from_slice(b"presence:");
        buf.extend_from_slice(player.as_bytes());
        buf.extend_from_slice(&(nickname.len() as u32).to_le_bytes());
        buf.extend_from_slice(nickname.as_bytes());
        buf.push(match claimed {
            PresenceStatus::Online => 0,
            PresenceStatus::Away => 1,
            PresenceStatus::Offline => 2,
        });
        match watching {
            Some(id) => {
                buf.push(1);
                buf.extend_from_slice(&id.0);
            }
            None => buf.push(0),
        }
        buf.extend_from_slice(&updated_at.to_le_bytes());
        buf
    }

    pub fn new(
        key: &SigningKey,
        nickname: String,
        claimed: PresenceStatus,
        updated_at: i64,
    ) -> SignedPresence {
        Self::watching_game(key, nickname, claimed, None, updated_at)
    }

    /// A heartbeat that also announces which game the player is watching.
    pub fn watching_game(
        key: &SigningKey,
        nickname: String,
        claimed: PresenceStatus,
        watching: Option<crate::identity::GameId>,
        updated_at: i64,
    ) -> SignedPresence {
        let player = key.verifying_key();
        let signature = key.sign(&Self::signing_bytes(
            &player, &nickname, claimed, watching, updated_at,
        ));
        SignedPresence {
            player,
            nickname,
            claimed,
            watching,
            updated_at,
            signature,
        }
    }

    pub fn player_id(&self) -> PlayerId {
        PlayerId::from(&self.player)
    }

    pub fn verify(&self) -> Result<(), String> {
        if self.nickname.len() > MAX_NICKNAME_LEN {
            return Err(format!("nickname longer than {MAX_NICKNAME_LEN} bytes"));
        }
        if self.updated_at <= 0 {
            return Err("presence timestamp must be positive".to_string());
        }
        verify_sig(
            &self.player,
            &Self::signing_bytes(
                &self.player,
                &self.nickname,
                self.claimed,
                self.watching,
                self.updated_at,
            ),
            &self.signature,
            "presence",
        )
    }

    /// The status as of `now`: a stale heartbeat decays to away, then offline,
    /// no matter what it claimed.
    pub fn status_at(&self, now: i64) -> PresenceStatus {
        let age = now - self.updated_at;
        if age > AWAY_WINDOW_MS {
            PresenceStatus::Offline
        } else if age > ONLINE_WINDOW_MS || self.claimed == PresenceStatus::Away {
            PresenceStatus::Away
        } else {
            self.claimed
        }
    }

    fn order_key(&self) -> (i64, [u8; 64]) {
        (self.updated_at, self.signature.to_bytes())
    }
}

/// Who is around.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceV1 {
    pub players: BTreeMap<PlayerId, SignedPresence>,
}

impl PresenceV1 {
    pub fn len(&self) -> usize {
        self.players.len()
    }

    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }

    /// A player's status as of `now`, defaulting to offline for anyone with no
    /// heartbeat at all.
    pub fn status_of(&self, player: PlayerId, now: i64) -> PresenceStatus {
        self.players
            .get(&player)
            .map(|p| p.status_at(now))
            .unwrap_or(PresenceStatus::Offline)
    }

    /// Everyone currently online or away, most recently seen first.
    pub fn active(&self, now: i64) -> Vec<&SignedPresence> {
        let mut out: Vec<&SignedPresence> = self
            .players
            .values()
            .filter(|p| p.status_at(now) != PresenceStatus::Offline)
            .collect();
        out.sort_by_key(|p| (std::cmp::Reverse(p.updated_at), p.player_id().0));
        out
    }

    /// Who is currently watching `game` — the pool a spectator can pick an
    /// opponent from.
    pub fn spectators_of(&self, game: crate::identity::GameId, now: i64) -> Vec<&SignedPresence> {
        let mut out: Vec<&SignedPresence> = self
            .players
            .values()
            .filter(|p| p.watching == Some(game) && p.status_at(now) != PresenceStatus::Offline)
            .collect();
        out.sort_by_key(|p| (std::cmp::Reverse(p.updated_at), p.player_id().0));
        out
    }

    /// Everyone available to be challenged: online or away, and not the caller.
    pub fn challengeable(&self, me: PlayerId, now: i64) -> Vec<&SignedPresence> {
        self.active(now)
            .into_iter()
            .filter(|p| p.player_id() != me)
            .collect()
    }

    pub fn online_count(&self, now: i64) -> usize {
        self.players
            .values()
            .filter(|p| p.status_at(now) == PresenceStatus::Online)
            .count()
    }

    fn absorb(&mut self, incoming: SignedPresence) {
        let id = incoming.player_id();
        match self.players.get(&id) {
            None => {
                self.players.insert(id, incoming);
            }
            Some(current) => {
                // Newest heartbeat wins — monotone in time, so convergent.
                if incoming.order_key() > current.order_key() {
                    self.players.insert(id, incoming);
                }
            }
        }
    }

    /// Keep the most recent heartbeats. Deterministic over the merged set.
    pub(crate) fn prune(&mut self) {
        if self.players.len() <= MAX_PRESENCE_ENTRIES {
            return;
        }
        let mut ids: Vec<PlayerId> = self.players.keys().copied().collect();
        ids.sort_by_key(|id| {
            let p = &self.players[id];
            (std::cmp::Reverse(p.updated_at), *id)
        });
        for id in ids.into_iter().skip(MAX_PRESENCE_ENTRIES) {
            self.players.remove(&id);
        }
    }
}

impl ComposableState for PresenceV1 {
    type ParentState = LobbyStateV1;
    type Summary = Vec<(PlayerId, i64)>;
    type Delta = Vec<SignedPresence>;
    type Parameters = LobbyParametersV1;

    fn verify(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
    ) -> Result<(), String> {
        if self.players.len() > MAX_PRESENCE_ENTRIES {
            return Err(format!(
                "presence holds {} entries, over the {MAX_PRESENCE_ENTRIES} cap",
                self.players.len()
            ));
        }
        for (id, entry) in &self.players {
            if &entry.player_id() != id {
                return Err("presence stored under the wrong player id".to_string());
            }
            entry.verify()?;
        }
        Ok(())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        self.players
            .iter()
            .map(|(id, p)| (*id, p.updated_at))
            .collect()
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let theirs: BTreeMap<PlayerId, i64> = old_summary.iter().copied().collect();
        let changed: Vec<SignedPresence> = self
            .players
            .iter()
            .filter(|(id, p)| {
                theirs
                    .get(id)
                    .map(|their_time| *their_time < p.updated_at)
                    .unwrap_or(true)
            })
            .map(|(_, p)| p.clone())
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
                entry.verify()?;
                self.absorb(entry.clone());
            }
        }
        Ok(())
    }
}
