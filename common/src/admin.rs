//! System administration: admins, announcements, shutdown and game takedowns.
//!
//! # Where authority comes from
//!
//! There is no administrator baked into the code or the contract parameters.
//! The **first** player to claim adminship gets it (a self-signed grant), and
//! every admin after that is granted by an existing one, forming a chain back
//! to that first claim.
//!
//! Two claims can race, so the winner is decided by a total order over the
//! *set* of claims — earliest `at`, then lowest key bytes — never by arrival
//! order. Every peer therefore computes the same root from the same data, in
//! any order, however many times it merges.
//!
//! # What this can and cannot enforce
//!
//! Be clear-eyed about the limits, because they follow from the choice of a
//! first-come root rather than a fixed one:
//!
//! * **Anyone can become the root** on a lobby where nobody has claimed yet —
//!   including someone who is not the operator. The claim is a race.
//! * **A game takedown is advisory.** Contracts cannot read each other's state
//!   (contract interoperability is not implemented in Freenet), so a game
//!   contract has no way to check whether a key is an admin of the lobby. A
//!   takedown marks the game as ended *in the lobby*, which is what clients
//!   display and search — but the two players can still exchange moves in the
//!   game contract itself, and those moves stay valid.
//! * **Shutdown is advisory** for the same reason: it is a flag clients honour
//!   by showing a notice. A modified client can ignore it.
//!
//! Making either binding would need the admin root fixed somewhere every
//! contract can see — its own parameters or a constant in the WASM — which is a
//! deliberately different trade-off (it fixes the operator at publish time).

use crate::game::setup::MAX_NICKNAME_LEN;
use crate::game::SIG_DOMAIN;
use crate::identity::{verify_sig, GameId, PlayerId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// Admins the system tracks.
pub const MAX_ADMINS: usize = 50;

/// Announcements kept at once; the oldest are evicted.
pub const MAX_ANNOUNCEMENTS: usize = 20;

/// Game takedowns retained.
pub const MAX_TAKEDOWNS: usize = 500;

pub const MAX_ANNOUNCEMENT_LEN: usize = 500;
pub const MAX_REASON_LEN: usize = 200;

/// A grant of admin rights.
///
/// The first grant in a system is *self-signed* (`granted_by == grantee`),
/// which is what "first to claim it" means concretely. Later grants are signed
/// by an existing admin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGrant {
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub grantee: VerifyingKey,
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub granted_by: VerifyingKey,
    /// Display name at grant time, so the admin list reads sensibly without a
    /// second lookup.
    pub nickname: String,
    pub at: i64,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

fn grant_signing_bytes(
    grantee: &VerifyingKey,
    granted_by: &VerifyingKey,
    nickname: &str,
    at: i64,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);
    buf.extend_from_slice(SIG_DOMAIN);
    buf.extend_from_slice(b"admin-grant:");
    buf.extend_from_slice(grantee.as_bytes());
    buf.extend_from_slice(granted_by.as_bytes());
    buf.extend_from_slice(&(nickname.len() as u32).to_le_bytes());
    buf.extend_from_slice(nickname.as_bytes());
    buf.extend_from_slice(&at.to_le_bytes());
    buf
}

impl AdminGrant {
    /// Claim adminship for yourself. Only valid when nobody else has claimed
    /// it first — see [`AdminsV1`].
    pub fn claim(key: &SigningKey, nickname: String, at: i64) -> AdminGrant {
        let me = key.verifying_key();
        let signature = key.sign(&grant_signing_bytes(&me, &me, &nickname, at));
        AdminGrant {
            grantee: me,
            granted_by: me,
            nickname,
            at,
            signature,
        }
    }

    /// Grant adminship to someone else. Signed by the granting admin.
    pub fn grant(
        admin: &SigningKey,
        grantee: VerifyingKey,
        nickname: String,
        at: i64,
    ) -> AdminGrant {
        let by = admin.verifying_key();
        let signature = admin.sign(&grant_signing_bytes(&grantee, &by, &nickname, at));
        AdminGrant {
            grantee,
            granted_by: by,
            nickname,
            at,
            signature,
        }
    }

    pub fn grantee_id(&self) -> PlayerId {
        PlayerId::from(&self.grantee)
    }

    pub fn is_self_claim(&self) -> bool {
        self.grantee == self.granted_by
    }

    /// Check the signature and the field bounds. Says the grant is authentic,
    /// *not* that the granter was entitled to make it — that is
    /// [`AdminsV1::check`]'s job, since it needs the whole set.
    pub fn verify_signature(&self) -> Result<(), String> {
        if self.nickname.len() > MAX_NICKNAME_LEN {
            return Err(format!("nickname longer than {MAX_NICKNAME_LEN} bytes"));
        }
        if self.at <= 0 {
            return Err("grant timestamp must be positive".to_string());
        }
        verify_sig(
            &self.granted_by,
            &grant_signing_bytes(&self.grantee, &self.granted_by, &self.nickname, self.at),
            &self.signature,
            "admin grant",
        )
    }

    /// Total order used to settle a race between competing self-claims.
    fn claim_order(&self) -> (i64, [u8; 32]) {
        (self.at, self.grantee.to_bytes())
    }
}

/// The set of admins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminsV1 {
    pub grants: BTreeMap<PlayerId, AdminGrant>,
}

impl AdminsV1 {
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    pub fn len(&self) -> usize {
        self.grants.len()
    }

    /// The winning self-claim: the root of the admin chain.
    pub fn root(&self) -> Option<&AdminGrant> {
        self.grants
            .values()
            .filter(|g| g.is_self_claim())
            .min_by_key(|g| g.claim_order())
    }

    pub fn is_admin(&self, player: PlayerId) -> bool {
        self.grants.contains_key(&player)
    }

    /// Admins in a stable order, root first.
    pub fn listed(&self) -> Vec<&AdminGrant> {
        let mut out: Vec<&AdminGrant> = self.grants.values().collect();
        out.sort_by_key(|g| (!g.is_self_claim(), g.at, g.grantee_id().0));
        out
    }

    fn absorb(&mut self, incoming: AdminGrant) {
        let id = incoming.grantee_id();
        match self.grants.get(&id) {
            None => {
                self.grants.insert(id, incoming);
            }
            Some(current) => {
                // Earliest grant wins, so re-granting cannot reorder history.
                if incoming.claim_order() < current.claim_order() {
                    self.grants.insert(id, incoming);
                }
            }
        }
    }

    /// Drop every grant that does not chain back to the winning root.
    ///
    /// A pure, idempotent function of the merged set. It has to exist because
    /// the root is decided by the set: if a later-discovered self-claim wins the
    /// race, grants descending from the loser lose their authority and must go.
    pub fn prune(&mut self) {
        let Some(root) = self.root().cloned() else {
            self.grants.clear();
            return;
        };

        // Grow the authorised set outward from the root, admitting a grant only
        // once its granter is already known to be an admin.
        let mut authorised: BTreeMap<PlayerId, AdminGrant> = BTreeMap::new();
        authorised.insert(root.grantee_id(), root.clone());

        loop {
            let mut added = false;
            for grant in self.grants.values() {
                let id = grant.grantee_id();
                if authorised.contains_key(&id) || grant.is_self_claim() {
                    continue;
                }
                let granter = PlayerId::from(&grant.granted_by);
                if authorised.contains_key(&granter) {
                    authorised.insert(id, grant.clone());
                    added = true;
                }
            }
            if !added {
                break;
            }
        }

        // Cap deterministically: root first, then oldest grants.
        if authorised.len() > MAX_ADMINS {
            let mut ids: Vec<PlayerId> = authorised.keys().copied().collect();
            ids.sort_by_key(|id| {
                let g = &authorised[id];
                (!g.is_self_claim(), g.at, *id)
            });
            for id in ids.into_iter().skip(MAX_ADMINS) {
                authorised.remove(&id);
            }
        }

        self.grants = authorised;
    }

    /// Reject a set that does not already satisfy the chain rule. Unlike
    /// [`prune`](Self::prune) this refuses rather than repairs, so a full-state
    /// PUT cannot smuggle in self-appointed admins.
    pub fn check(&self) -> Result<(), String> {
        if self.grants.len() > MAX_ADMINS {
            return Err(format!(
                "{} admins, over the {MAX_ADMINS} cap",
                self.grants.len()
            ));
        }
        for (id, grant) in &self.grants {
            if &grant.grantee_id() != id {
                return Err("admin grant stored under the wrong player id".to_string());
            }
            grant.verify_signature()?;
        }

        if self.grants.is_empty() {
            return Ok(());
        }

        // Exactly one self-claim may be authoritative, and it must be the
        // winner of the total order; any other self-claim is a second root.
        let self_claims: Vec<&AdminGrant> =
            self.grants.values().filter(|g| g.is_self_claim()).collect();
        if self_claims.is_empty() {
            return Err("admin set has no root claim".to_string());
        }
        if self_claims.len() > 1 {
            return Err("admin set has more than one root claim".to_string());
        }

        let root = self_claims[0];
        let mut pruned = self.clone();
        pruned.prune();
        if &pruned != self {
            return Err("admin set contains grants that do not chain back to the root".to_string());
        }
        let _ = root;
        Ok(())
    }
}

/// A public notice from an admin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Announcement {
    /// Content-derived id, so the same notice merges rather than duplicating.
    pub id: [u8; 16],
    pub text: String,
    pub at: i64,
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub author: VerifyingKey,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

fn announcement_signing_bytes(text: &str, at: i64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64 + text.len());
    buf.extend_from_slice(SIG_DOMAIN);
    buf.extend_from_slice(b"announce:");
    buf.extend_from_slice(&(text.len() as u32).to_le_bytes());
    buf.extend_from_slice(text.as_bytes());
    buf.extend_from_slice(&at.to_le_bytes());
    buf
}

impl Announcement {
    pub fn new(admin: &SigningKey, text: String, at: i64) -> Announcement {
        let bytes = announcement_signing_bytes(&text, at);
        let mut id = [0u8; 16];
        id.copy_from_slice(&blake3::hash(&bytes).as_bytes()[..16]);
        Announcement {
            id,
            text,
            at,
            author: admin.verifying_key(),
            signature: admin.sign(&bytes),
        }
    }

    pub fn author_id(&self) -> PlayerId {
        PlayerId::from(&self.author)
    }

    /// Authentic, within bounds, and written by an admin.
    pub fn verify(&self, admins: &AdminsV1) -> Result<(), String> {
        if self.text.trim().is_empty() {
            return Err("announcement must not be blank".to_string());
        }
        if self.text.len() > MAX_ANNOUNCEMENT_LEN {
            return Err(format!(
                "announcement longer than {MAX_ANNOUNCEMENT_LEN} bytes"
            ));
        }
        if self.at <= 0 {
            return Err("announcement timestamp must be positive".to_string());
        }
        if !admins.is_admin(self.author_id()) {
            return Err("announcements may only be published by an admin".to_string());
        }
        let bytes = announcement_signing_bytes(&self.text, self.at);
        let mut expected = [0u8; 16];
        expected.copy_from_slice(&blake3::hash(&bytes).as_bytes()[..16]);
        if expected != self.id {
            return Err("announcement id does not match its content".to_string());
        }
        verify_sig(&self.author, &bytes, &self.signature, "announcement")
    }
}

/// An admin marking a game as taken down.
///
/// Advisory: clients hide or flag the game, but the game's own contract knows
/// nothing about this and its players can still move. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Takedown {
    pub game_id: GameId,
    pub reason: String,
    pub at: i64,
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub author: VerifyingKey,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

fn takedown_signing_bytes(game_id: &GameId, reason: &str, at: i64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(96 + reason.len());
    buf.extend_from_slice(SIG_DOMAIN);
    buf.extend_from_slice(b"takedown:");
    buf.extend_from_slice(&game_id.0);
    buf.extend_from_slice(&(reason.len() as u32).to_le_bytes());
    buf.extend_from_slice(reason.as_bytes());
    buf.extend_from_slice(&at.to_le_bytes());
    buf
}

impl Takedown {
    pub fn new(admin: &SigningKey, game_id: GameId, reason: String, at: i64) -> Takedown {
        Takedown {
            game_id,
            reason: reason.clone(),
            at,
            author: admin.verifying_key(),
            signature: admin.sign(&takedown_signing_bytes(&game_id, &reason, at)),
        }
    }

    pub fn author_id(&self) -> PlayerId {
        PlayerId::from(&self.author)
    }

    pub fn verify(&self, admins: &AdminsV1) -> Result<(), String> {
        if self.reason.len() > MAX_REASON_LEN {
            return Err(format!("reason longer than {MAX_REASON_LEN} bytes"));
        }
        if self.at <= 0 {
            return Err("takedown timestamp must be positive".to_string());
        }
        if !admins.is_admin(self.author_id()) {
            return Err("games may only be taken down by an admin".to_string());
        }
        verify_sig(
            &self.author,
            &takedown_signing_bytes(&self.game_id, &self.reason, self.at),
            &self.signature,
            "takedown",
        )
    }
}

/// Whether the system is marked available, and why not if not.
///
/// Advisory: clients show the notice. Nothing in any contract refuses work
/// because of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceState {
    pub available: bool,
    pub message: String,
    pub at: i64,
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub author: VerifyingKey,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

fn service_signing_bytes(available: bool, message: &str, at: i64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64 + message.len());
    buf.extend_from_slice(SIG_DOMAIN);
    buf.extend_from_slice(b"service:");
    buf.push(u8::from(available));
    buf.extend_from_slice(&(message.len() as u32).to_le_bytes());
    buf.extend_from_slice(message.as_bytes());
    buf.extend_from_slice(&at.to_le_bytes());
    buf
}

impl ServiceState {
    pub fn new(admin: &SigningKey, available: bool, message: String, at: i64) -> ServiceState {
        ServiceState {
            available,
            message: message.clone(),
            at,
            author: admin.verifying_key(),
            signature: admin.sign(&service_signing_bytes(available, &message, at)),
        }
    }

    pub fn author_id(&self) -> PlayerId {
        PlayerId::from(&self.author)
    }

    pub fn verify(&self, admins: &AdminsV1) -> Result<(), String> {
        if self.message.len() > MAX_ANNOUNCEMENT_LEN {
            return Err("service message too long".to_string());
        }
        if self.at <= 0 {
            return Err("service timestamp must be positive".to_string());
        }
        if !admins.is_admin(self.author_id()) {
            return Err("only an admin may change service state".to_string());
        }
        verify_sig(
            &self.author,
            &service_signing_bytes(self.available, &self.message, self.at),
            &self.signature,
            "service state",
        )
    }

    /// Newest statement wins, with a deterministic tiebreak.
    fn order(&self) -> (i64, [u8; 64]) {
        (self.at, self.signature.to_bytes())
    }
}

/// All administration state, carried by the lobby.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdministrationV1 {
    pub admins: AdminsV1,
    pub announcements: BTreeMap<[u8; 16], Announcement>,
    pub takedowns: BTreeMap<GameId, Takedown>,
    pub service: Option<ServiceState>,
}

impl AdministrationV1 {
    pub fn is_admin(&self, player: PlayerId) -> bool {
        self.admins.is_admin(player)
    }

    /// Announcements newest first.
    pub fn recent_announcements(&self) -> Vec<&Announcement> {
        let mut out: Vec<&Announcement> = self.announcements.values().collect();
        out.sort_by_key(|a| (std::cmp::Reverse(a.at), a.id));
        out
    }

    pub fn is_taken_down(&self, game: &GameId) -> bool {
        self.takedowns.contains_key(game)
    }

    /// Whether clients should present the system as available.
    pub fn available(&self) -> bool {
        self.service.as_ref().map(|s| s.available).unwrap_or(true)
    }

    pub fn service_message(&self) -> Option<String> {
        self.service
            .as_ref()
            .filter(|s| !s.available && !s.message.trim().is_empty())
            .map(|s| s.message.clone())
    }

    pub(crate) fn absorb_grant(&mut self, grant: AdminGrant) {
        self.admins.absorb(grant);
    }

    pub(crate) fn absorb_announcement(&mut self, a: Announcement) {
        self.announcements.entry(a.id).or_insert(a);
    }

    pub(crate) fn absorb_takedown(&mut self, t: Takedown) {
        match self.takedowns.get(&t.game_id) {
            None => {
                self.takedowns.insert(t.game_id, t);
            }
            Some(current) => {
                // Earliest takedown wins; a total order keeps peers in step.
                if (t.at, t.signature.to_bytes()) < (current.at, current.signature.to_bytes()) {
                    self.takedowns.insert(t.game_id, t);
                }
            }
        }
    }

    pub(crate) fn absorb_service(&mut self, s: ServiceState) {
        match &self.service {
            None => self.service = Some(s),
            Some(current) => {
                if s.order() > current.order() {
                    self.service = Some(s);
                }
            }
        }
    }

    /// Deterministic cleanup: settle the admin chain, then drop anything no
    /// longer authorised, then apply the caps.
    pub(crate) fn prune(&mut self) {
        self.admins.prune();

        // A grant being revoked retroactively (because its chain lost the root
        // race) also invalidates what that admin published.
        self.announcements
            .retain(|_, a| self.admins.is_admin(a.author_id()));
        self.takedowns
            .retain(|_, t| self.admins.is_admin(t.author_id()));
        if let Some(service) = &self.service {
            if !self.admins.is_admin(service.author_id()) {
                self.service = None;
            }
        }

        if self.announcements.len() > MAX_ANNOUNCEMENTS {
            let mut ids: Vec<[u8; 16]> = self.announcements.keys().copied().collect();
            ids.sort_by_key(|id| {
                let a = &self.announcements[id];
                (std::cmp::Reverse(a.at), *id)
            });
            for id in ids.into_iter().skip(MAX_ANNOUNCEMENTS) {
                self.announcements.remove(&id);
            }
        }
        if self.takedowns.len() > MAX_TAKEDOWNS {
            let mut ids: Vec<GameId> = self.takedowns.keys().copied().collect();
            ids.sort_by_key(|id| {
                let t = &self.takedowns[id];
                (std::cmp::Reverse(t.at), *id)
            });
            for id in ids.into_iter().skip(MAX_TAKEDOWNS) {
                self.takedowns.remove(&id);
            }
        }
    }

    /// Reject state that is not already well-formed.
    pub(crate) fn check(&self) -> Result<(), String> {
        self.admins.check()?;
        for (id, a) in &self.announcements {
            if &a.id != id {
                return Err("announcement stored under the wrong id".to_string());
            }
            a.verify(&self.admins)?;
        }
        for (id, t) in &self.takedowns {
            if &t.game_id != id {
                return Err("takedown stored under the wrong game id".to_string());
            }
            t.verify(&self.admins)?;
        }
        if let Some(service) = &self.service {
            service.verify(&self.admins)?;
        }
        Ok(())
    }
}

/// One administrative action, as it travels on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminAction {
    Grant(AdminGrant),
    Announce(Announcement),
    TakeDown(Takedown),
    Service(ServiceState),
}

impl freenet_scaffold::ComposableState for AdministrationV1 {
    type ParentState = crate::lobby::LobbyStateV1;
    type Summary = (usize, usize, usize, i64);
    type Delta = Vec<AdminAction>;
    type Parameters = crate::lobby::LobbyParametersV1;

    fn verify(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
    ) -> Result<(), String> {
        self.check()
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        (
            self.admins.len(),
            self.announcements.len(),
            self.takedowns.len(),
            self.service.as_ref().map(|s| s.at).unwrap_or(0),
        )
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old: &Self::Summary,
    ) -> Option<Self::Delta> {
        // Counts are a coarse summary, so when anything differs we ship the
        // whole administrative set. It is small (tens of entries) and the merge
        // is idempotent, so re-sending costs little and cannot corrupt state.
        let mine = (
            self.admins.len(),
            self.announcements.len(),
            self.takedowns.len(),
            self.service.as_ref().map(|s| s.at).unwrap_or(0),
        );
        if mine == *old {
            return None;
        }
        let mut actions: Vec<AdminAction> = Vec::new();
        for g in self.admins.grants.values() {
            actions.push(AdminAction::Grant(g.clone()));
        }
        for a in self.announcements.values() {
            actions.push(AdminAction::Announce(a.clone()));
        }
        for t in self.takedowns.values() {
            actions.push(AdminAction::TakeDown(t.clone()));
        }
        if let Some(s) = &self.service {
            actions.push(AdminAction::Service(s.clone()));
        }
        if actions.is_empty() {
            None
        } else {
            Some(actions)
        }
    }

    fn apply_delta(
        &mut self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        let Some(actions) = delta else {
            return Ok(());
        };

        // Grants first: everything else is only admissible once the admin set
        // it depends on is in place. Within grants, signature authenticity is
        // all that can be checked here — the chain is settled by `prune` over
        // the merged set.
        for action in actions {
            if let AdminAction::Grant(g) = action {
                g.verify_signature()?;
                self.absorb_grant(g.clone());
            }
        }
        self.admins.prune();

        for action in actions {
            match action {
                AdminAction::Grant(_) => {}
                AdminAction::Announce(a) => {
                    // Silently skip rather than failing the whole delta: a
                    // notice from someone whose grant lost the root race is not
                    // an attack, just stale.
                    if a.verify(&self.admins).is_ok() {
                        self.absorb_announcement(a.clone());
                    }
                }
                AdminAction::TakeDown(t) => {
                    if t.verify(&self.admins).is_ok() {
                        self.absorb_takedown(t.clone());
                    }
                }
                AdminAction::Service(s) => {
                    if s.verify(&self.admins).is_ok() {
                        self.absorb_service(s.clone());
                    }
                }
            }
        }
        Ok(())
    }
}
