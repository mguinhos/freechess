//! A player's public profile: their chosen nickname and their certified game
//! history.
//!
//! One instance per player, parameterized by their verifying key — so the
//! contract key is derived from the key the player already owns, and the
//! profile is reachable by anyone who knows their [`PlayerId`].
//!
//! Like everything else in the app this is public, replicated state. The only
//! thing that stays on the device is the private signing key, which lives in
//! the delegate.

use crate::certificate::GameCertificate;
use crate::elo::{self, RatedGame, STARTING_ELO};
use crate::game::setup::MAX_NICKNAME_LEN;
use crate::game::SIG_DOMAIN;
use crate::identity::{signature_digest, verify_sig, GameId, PlayerId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold_macro::composable;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// Certified games a profile keeps. Older ones remain in the shared archive.
pub const PROFILE_HISTORY_SIZE: usize = 200;

/// Minimum nickname length, so a profile is identifiable.
pub const MIN_NICKNAME_LEN: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerParametersV1 {
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub player: VerifyingKey,
}

impl PlayerParametersV1 {
    pub fn player_id(&self) -> PlayerId {
        PlayerId::from(&self.player)
    }
}

/// Validate a nickname. Applied identically on every peer, so a name that one
/// client accepts cannot be rejected by another.
pub fn validate_nickname(nickname: &str) -> Result<(), String> {
    let trimmed = nickname.trim();
    if trimmed.len() < MIN_NICKNAME_LEN {
        return Err(format!(
            "nickname must be at least {MIN_NICKNAME_LEN} characters"
        ));
    }
    if nickname.len() > MAX_NICKNAME_LEN {
        return Err(format!("nickname must be at most {MAX_NICKNAME_LEN} bytes"));
    }
    if trimmed != nickname {
        return Err("nickname must not have leading or trailing whitespace".to_string());
    }
    // Control characters would let a nickname disrupt the UI or forge
    // line-based output; everything printable is allowed, including emoji.
    if nickname.chars().any(|c| c.is_control()) {
        return Err("nickname must not contain control characters".to_string());
    }
    Ok(())
}

/// The player's self-chosen display name.
///
/// Nicknames are deliberately *not* unique. Uniqueness would need a global
/// registry with a first-come-first-served race, which is exactly the kind of
/// global coordination this platform avoids. Identity is the [`PlayerId`]; the
/// nickname is a label, shown alongside a short id wherever it matters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedProfile {
    pub nickname: String,
    pub updated_at: i64,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl SignedProfile {
    fn signing_bytes(player: &VerifyingKey, nickname: &str, updated_at: i64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(96);
        buf.extend_from_slice(SIG_DOMAIN);
        buf.extend_from_slice(b"profile:");
        buf.extend_from_slice(player.as_bytes());
        buf.extend_from_slice(&(nickname.len() as u32).to_le_bytes());
        buf.extend_from_slice(nickname.as_bytes());
        buf.extend_from_slice(&updated_at.to_le_bytes());
        buf
    }

    pub fn new(key: &SigningKey, nickname: String, updated_at: i64) -> SignedProfile {
        let signature = key.sign(&Self::signing_bytes(
            &key.verifying_key(),
            &nickname,
            updated_at,
        ));
        SignedProfile {
            nickname,
            updated_at,
            signature,
        }
    }

    pub fn verify(&self, params: &PlayerParametersV1) -> Result<(), String> {
        validate_nickname(&self.nickname)?;
        verify_sig(
            &params.player,
            &Self::signing_bytes(&params.player, &self.nickname, self.updated_at),
            &self.signature,
            "profile",
        )
    }

    /// Last-writer-wins by timestamp, with signature bytes as a deterministic
    /// tiebreak so two edits stamped the same millisecond still converge.
    fn order_key(&self) -> (i64, [u8; 64]) {
        (self.updated_at, self.signature.to_bytes())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileV1(pub Option<SignedProfile>);

impl ProfileV1 {
    pub fn get(&self) -> Option<&SignedProfile> {
        self.0.as_ref()
    }

    /// The display name, falling back to a short id for a player who has not
    /// set one.
    pub fn nickname_or(&self, id: PlayerId) -> String {
        match &self.0 {
            Some(p) => p.nickname.clone(),
            None => format!("player-{}", &id.to_base58()[..6]),
        }
    }

    fn absorb(&mut self, incoming: SignedProfile) {
        match &self.0 {
            None => self.0 = Some(incoming),
            Some(current) => {
                if incoming.order_key() > current.order_key() {
                    self.0 = Some(incoming);
                }
            }
        }
    }
}

impl ComposableState for ProfileV1 {
    type ParentState = PlayerStateV1;
    /// A fingerprint of the profile we hold, not just its timestamp: `absorb`
    /// breaks a tie on `updated_at` by signature bytes, so a summary of the
    /// timestamp alone left two profiles stamped the same millisecond unable to
    /// reconcile.
    type Summary = Option<u64>;
    type Delta = SignedProfile;
    type Parameters = PlayerParametersV1;

    fn verify(&self, _parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        match &self.0 {
            Some(p) => p.verify(params),
            None => Ok(()),
        }
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        self.0.as_ref().map(|p| signature_digest(&p.signature))
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let current = self.0.as_ref()?;
        if *old_summary == Some(signature_digest(&current.signature)) {
            return None;
        }
        Some(current.clone())
    }

    fn apply_delta(
        &mut self,
        _parent: &Self::ParentState,
        params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(profile) = delta {
            profile.verify(params)?;
            self.absorb(profile.clone());
        }
        Ok(())
    }
}

/// The player's certified games.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryV1 {
    pub games: BTreeMap<GameId, GameCertificate>,
}

impl HistoryV1 {
    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    /// Games in the order they were played — the order the rating replays in.
    pub fn chronological(&self) -> Vec<&GameCertificate> {
        let mut out: Vec<&GameCertificate> = self.games.values().collect();
        out.sort_by_key(|c| c.order_key());
        out
    }

    /// Newest first, for display.
    pub fn recent(&self) -> Vec<&GameCertificate> {
        let mut out = self.chronological();
        out.reverse();
        out
    }

    /// The player's rating, replayed from their certified games.
    ///
    /// Deterministic: the games are sorted by a total order first, and
    /// [`elo`] is pure integer arithmetic, so every peer computes the same
    /// number.
    pub fn rating(&self, player: PlayerId) -> i32 {
        let history: Vec<RatedGame> = self
            .chronological()
            .into_iter()
            .filter_map(|c| {
                Some(RatedGame {
                    opponent_rating: c.opponent_rating_for(player)?,
                    score: c.score_for(player)?,
                })
            })
            .collect();
        elo::rating_from_history(&history)
    }

    /// Wins, draws and losses.
    pub fn record(&self, player: PlayerId) -> (u32, u32, u32) {
        let mut record = (0, 0, 0);
        for cert in self.games.values() {
            match cert.score_for(player) {
                Some(elo::Score::Win) => record.0 += 1,
                Some(elo::Score::Draw) => record.1 += 1,
                Some(elo::Score::Loss) => record.2 += 1,
                None => {}
            }
        }
        record
    }

    fn absorb(&mut self, cert: GameCertificate) {
        match self.games.get(&cert.game_id) {
            None => {
                self.games.insert(cert.game_id, cert);
            }
            Some(existing) => {
                if cert.white_signature.to_bytes() < existing.white_signature.to_bytes() {
                    self.games.insert(cert.game_id, cert);
                }
            }
        }
    }

    /// Keep the newest [`PROFILE_HISTORY_SIZE`] games. Deterministic eviction:
    /// pure in the merged set, idempotent, order-independent.
    fn enforce_cap(&mut self) {
        if self.games.len() <= PROFILE_HISTORY_SIZE {
            return;
        }
        let mut ids: Vec<GameId> = self.games.keys().copied().collect();
        ids.sort_by_key(|id| {
            let c = &self.games[id];
            (std::cmp::Reverse(c.finished_at), *id)
        });
        for id in ids.into_iter().skip(PROFILE_HISTORY_SIZE) {
            self.games.remove(&id);
        }
    }
}

impl ComposableState for HistoryV1 {
    type ParentState = PlayerStateV1;
    /// Game id *and* a fingerprint of the certificate held for it. A bare set of
    /// ids meant a rival certificate for a game the peer already had never
    /// shipped, so the equivocation tiebreak never ran and the two kept
    /// different history.
    type Summary = Vec<(GameId, u64)>;
    type Delta = Vec<GameCertificate>;
    type Parameters = PlayerParametersV1;

    fn verify(&self, _parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        if self.games.len() > PROFILE_HISTORY_SIZE {
            return Err(format!(
                "history holds {} games, over the {PROFILE_HISTORY_SIZE} cap",
                self.games.len()
            ));
        }
        let me = params.player_id();
        for (id, cert) in &self.games {
            if &cert.game_id != id {
                return Err("certificate stored under the wrong game id".to_string());
            }
            cert.verify()?;
            // A profile may only contain games this player actually played —
            // otherwise anyone could pad their history with strangers' wins.
            if !cert.involves(me) {
                return Err(format!(
                    "history contains game {id}, which this player did not take part in"
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
            let me = params.player_id();
            for cert in certs {
                cert.verify()?;
                if !cert.involves(me) {
                    return Err(format!(
                        "refusing to file game {} into a profile that did not play it",
                        cert.game_id
                    ));
                }
                self.absorb(cert.clone());
            }
        }
        Ok(())
    }
}

/// A player's public profile contract.
#[composable(post_apply_delta = "prune")]
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
pub struct PlayerStateV1 {
    pub profile: ProfileV1,
    pub history: HistoryV1,
}

impl PlayerStateV1 {
    pub fn prune(&mut self, _params: &PlayerParametersV1) -> Result<(), String> {
        self.history.enforce_cap();
        Ok(())
    }

    pub fn nickname(&self, id: PlayerId) -> String {
        self.profile.nickname_or(id)
    }

    pub fn rating(&self, id: PlayerId) -> i32 {
        if self.history.is_empty() {
            return STARTING_ELO;
        }
        self.history.rating(id)
    }

    pub fn games_played(&self) -> u32 {
        self.history.len() as u32
    }
}
