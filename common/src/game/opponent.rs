//! The second player's slot — the mechanism that closes a game to exactly two
//! participants.
//!
//! # Why the seat is countersigned, and not raced for
//!
//! Taking the seat is a two-step handshake: a challenger publishes a
//! [`SignedJoin`] as an *offer*, and the seat is only filled once the creator
//! countersigns exactly one of them ([`SignedAcceptance`]). Until that
//! countersignature exists the game has no opponent, and therefore no legal
//! move.
//!
//! The obvious design — let the earliest join win — is unsound here, and was
//! the original bug. The ordering key would be `(joined_at, key)`, but
//! `joined_at` is the *joiner's own* unverifiable claim, so an attacker simply
//! signs `joined_at = 1` and wins every race. Worse, they win it
//! **retroactively**: the merge is deliberately independent of arrival order (a
//! peer that learns of the attacker last must still reach the same answer), so
//! a backdated join lands in the middle of a game in progress, evicts the
//! seated player, and takes every move they had signed down with them.
//!
//! "Ignore whoever turns up late" is not available as a fix: arrival order
//! differs per peer, so a rule that depends on it splits the network
//! permanently. The eviction rule has to be a deterministic function of the
//! merged *set*, which means the set has to contain something an attacker
//! cannot produce. The creator's key is exactly that — it is a contract
//! parameter, so it is baked into the contract address and cannot be forged or
//! swapped. Anchoring the seat to a creator signature removes the race instead
//! of trying to referee it.
//!
//! Convergence still holds if the creator misbehaves: a creator who signs two
//! acceptances has them settled by a total order over the acceptance signature
//! bytes, which every peer computes identically from the same set.
//!
//! # Offers are proof-of-work bound
//!
//! Offers accumulate until one is accepted, so they need a cost, for the same
//! reason game creation does. Each carries a nonce whose digest over
//! `(game_id, player)` must have [`POW_DIFFICULTY_BITS`] leading zeros. The
//! offer list is also capped at [`MAX_JOIN_OFFERS`], keeping the lowest keys —
//! a deterministic rule, not an arrival-order one.
//!
//! Residual, and deliberately not papered over: a flooder willing to pay the
//! work can still fill an open game's offer list and keep an honest challenger
//! from being seen. They cannot take the seat, and cannot touch a game already
//! under way. A direct challenge is immune, since only the invited key may
//! offer at all.

use super::setup::{leading_zero_bits, POW_DIFFICULTY_BITS};
use super::{ChessGameParametersV1, ChessGameStateV1, SIG_DOMAIN};
use crate::identity::{signature_digest, verify_sig, GameId, PlayerId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::setup::MAX_NICKNAME_LEN;

/// How many un-accepted offers a game keeps. Bounded because they are held
/// until one is accepted; the cap keeps the lowest keys, which is a function of
/// the set rather than of arrival order.
pub const MAX_JOIN_OFFERS: usize = 16;

/// A challenger's signed offer to take the open seat.
///
/// An offer alone does not seat anyone — see [`SignedAcceptance`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedJoin {
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub player: VerifyingKey,
    /// Unix milliseconds the challenger claims to have offered at.
    ///
    /// Self-asserted and therefore never used to decide *who* gets the seat.
    /// It only feeds the clock, floored at the game's creation time, and only
    /// after the creator has countersigned this exact value.
    pub joined_at: i64,
    /// Display name, same role as the creator's copy in the setup.
    pub nickname: String,
    /// Proof-of-work nonce over `(game_id, player)`. Makes filling the offer
    /// list cost something.
    pub pow_nonce: u64,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl SignedJoin {
    /// Digest the proof-of-work must satisfy. Covers the player so a nonce
    /// mined for one key cannot be reused by another, and the game so it cannot
    /// be reused across games.
    fn pow_digest(game_id: &GameId, player: &VerifyingKey, pow_nonce: u64) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SIG_DOMAIN);
        hasher.update(b"joinpow:");
        hasher.update(&game_id.0);
        hasher.update(player.as_bytes());
        hasher.update(&pow_nonce.to_le_bytes());
        *hasher.finalize().as_bytes()
    }

    fn signing_bytes(
        game_id: &GameId,
        player: &VerifyingKey,
        joined_at: i64,
        nickname: &str,
        pow_nonce: u64,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(104);
        buf.extend_from_slice(SIG_DOMAIN);
        buf.extend_from_slice(b"join:");
        buf.extend_from_slice(&game_id.0);
        buf.extend_from_slice(player.as_bytes());
        buf.extend_from_slice(&joined_at.to_le_bytes());
        buf.extend_from_slice(&pow_nonce.to_le_bytes());
        buf.extend_from_slice(&(nickname.len() as u32).to_le_bytes());
        buf.extend_from_slice(nickname.as_bytes());
        buf
    }

    /// Mine and sign an offer for the open seat of `game_id`. The search runs
    /// on the challenger's client, never in the contract.
    pub fn new(key: &SigningKey, game_id: &GameId, joined_at: i64, nickname: String) -> SignedJoin {
        let player = key.verifying_key();
        let mut pow_nonce = 0u64;
        while leading_zero_bits(&Self::pow_digest(game_id, &player, pow_nonce))
            < POW_DIFFICULTY_BITS
        {
            pow_nonce = pow_nonce.wrapping_add(1);
        }
        let signature = key.sign(&Self::signing_bytes(
            game_id, &player, joined_at, &nickname, pow_nonce,
        ));
        SignedJoin {
            player,
            joined_at,
            nickname,
            pow_nonce,
            signature,
        }
    }

    pub fn player_id(&self) -> PlayerId {
        PlayerId::from(&self.player)
    }

    pub fn verify(&self, params: &ChessGameParametersV1) -> Result<(), String> {
        // A player cannot face themselves — that would let one key control both
        // sides and manufacture rated results at will.
        if self.player == params.creator {
            return Err("the creator cannot join their own game".to_string());
        }
        if self.nickname.len() > MAX_NICKNAME_LEN {
            return Err(format!("nickname longer than {MAX_NICKNAME_LEN} bytes"));
        }
        if self.joined_at <= 0 {
            return Err("joined_at must be positive".to_string());
        }
        let digest = Self::pow_digest(&params.game_id, &self.player, self.pow_nonce);
        if leading_zero_bits(&digest) < POW_DIFFICULTY_BITS {
            return Err("join does not carry enough proof-of-work".to_string());
        }
        verify_sig(
            &self.player,
            &Self::signing_bytes(
                &params.game_id,
                &self.player,
                self.joined_at,
                &self.nickname,
                self.pow_nonce,
            ),
            &self.signature,
            "join",
        )
    }

    /// Only the player a direct challenge names may offer on it.
    fn check_invitation(&self, parent: &ChessGameStateV1) -> Result<(), String> {
        let Some(setup) = parent.setup.get() else {
            return Ok(());
        };
        match setup.setup.challenged {
            Some(invited) if invited != self.player => {
                Err("this game is a direct challenge to a different player".to_string())
            }
            _ => Ok(()),
        }
    }
}

/// The invited player's refusal of a direct challenge.
///
/// A challenge that can only be accepted leaves the challenger waiting on
/// someone who has already decided not to play, with no way to tell that from
/// someone who simply has not looked yet. Declining is published rather than
/// kept on the device that clicked it, for exactly that reason: the useful part
/// is that the *other* person finds out.
///
/// Only the invited player may decline, so this exists only for direct
/// challenges — an open game has nobody in particular to refuse it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDecline {
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub player: VerifyingKey,
    pub at: i64,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

fn decline_signing_bytes(game_id: &GameId, player: &VerifyingKey, at: i64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(80);
    buf.extend_from_slice(SIG_DOMAIN);
    buf.extend_from_slice(b"decline:");
    buf.extend_from_slice(&game_id.0);
    buf.extend_from_slice(player.as_bytes());
    buf.extend_from_slice(&at.to_le_bytes());
    buf
}

impl SignedDecline {
    pub fn new(key: &SigningKey, game_id: &GameId, at: i64) -> SignedDecline {
        let player = key.verifying_key();
        SignedDecline {
            player,
            at,
            signature: key.sign(&decline_signing_bytes(game_id, &player, at)),
        }
    }

    pub fn player_id(&self) -> PlayerId {
        PlayerId::from(&self.player)
    }

    pub fn verify(
        &self,
        params: &ChessGameParametersV1,
        challenged: Option<VerifyingKey>,
    ) -> Result<(), String> {
        let Some(invited) = challenged else {
            return Err("only a direct challenge can be declined".to_string());
        };
        if self.player != invited {
            return Err("only the challenged player may decline".to_string());
        }
        if self.at <= 0 {
            return Err("decline timestamp must be positive".to_string());
        }
        verify_sig(
            &self.player,
            &decline_signing_bytes(&params.game_id, &self.player, self.at),
            &self.signature,
            "decline",
        )
    }
}

/// The creator's countersignature seating one challenger.
///
/// This is the whole authorization story for the second seat. The creator's key
/// lives in the contract parameters, so it is part of the contract address:
/// nobody else can produce this value, and no later message can replace the key
/// that validates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedAcceptance {
    /// The offer being accepted, carried whole so the seat is self-contained —
    /// a peer can validate it without having seen the offer separately.
    pub join: SignedJoin,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl SignedAcceptance {
    /// Covers the offer's own signature, which in turn covers every field of
    /// the offer. So accepting binds the creator to that exact challenger, at
    /// that exact claimed time, under that exact nickname — no field can be
    /// swapped afterwards under the same acceptance.
    fn signing_bytes(game_id: &GameId, join: &SignedJoin) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(SIG_DOMAIN);
        buf.extend_from_slice(b"accept:");
        buf.extend_from_slice(&game_id.0);
        buf.extend_from_slice(join.player.as_bytes());
        buf.extend_from_slice(&join.signature.to_bytes());
        buf
    }

    /// Countersign an offer. Only meaningful when `key` is the creator; any
    /// other signer fails [`verify`](Self::verify) on every peer.
    pub fn new(key: &SigningKey, game_id: &GameId, join: SignedJoin) -> SignedAcceptance {
        let signature = key.sign(&Self::signing_bytes(game_id, &join));
        SignedAcceptance { join, signature }
    }

    pub fn verify(&self, params: &ChessGameParametersV1) -> Result<(), String> {
        self.join.verify(params)?;
        verify_sig(
            &params.creator,
            &Self::signing_bytes(&params.game_id, &self.join),
            &self.signature,
            "seat acceptance",
        )
    }

    /// Total order settling a creator who countersigned two different offers.
    /// Deterministic and independent of arrival order.
    fn order_key(&self) -> [u8; 64] {
        self.signature.to_bytes()
    }

    fn wins_over(&self, other: &SignedAcceptance) -> bool {
        self.order_key() < other.order_key()
    }
}

/// The opponent seat: offers on the table, and the countersigned seat itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpponentSlotV1 {
    /// Offers awaiting the creator's decision, keyed by the challenger's key
    /// bytes. Dropped entirely once the seat is filled.
    #[serde(default)]
    pub offers: BTreeMap<[u8; 32], SignedJoin>,
    /// The seat, once countersigned. Immutable afterwards.
    #[serde(default)]
    pub seated: Option<SignedAcceptance>,
    /// The invited player's refusal, for a direct challenge they do not want.
    #[serde(default)]
    pub declined: Option<SignedDecline>,
}

impl OpponentSlotV1 {
    /// The seated opponent, or `None` while the seat is still open. A game with
    /// `None` here has no second player and therefore no legal move.
    pub fn get(&self) -> Option<&SignedJoin> {
        self.seated.as_ref().map(|a| &a.join)
    }

    /// Whether the invited player has refused this challenge.
    pub fn is_declined(&self) -> bool {
        self.declined.is_some()
    }

    pub fn accepted(&self) -> Option<&SignedAcceptance> {
        self.seated.as_ref()
    }

    /// Offers the creator could still accept, in a deterministic order.
    pub fn pending_offers(&self) -> Vec<&SignedJoin> {
        self.offers.values().collect()
    }

    /// Whether `player` has an offer on the table that has not been seated.
    pub fn has_offer_from(&self, player: &VerifyingKey) -> bool {
        self.offers.contains_key(player.as_bytes())
    }

    fn absorb_offer(&mut self, incoming: SignedJoin) {
        // A challenger who signs two offers is settled by signature bytes, so
        // the map is a function of the merged set either way.
        match self.offers.get(incoming.player.as_bytes()) {
            Some(existing) if existing.signature.to_bytes() <= incoming.signature.to_bytes() => {}
            _ => {
                self.offers.insert(*incoming.player.as_bytes(), incoming);
            }
        }
    }

    fn absorb_decline(&mut self, incoming: SignedDecline) {
        // Only one key can ever produce a valid decline, so this settles the
        // one case left: that key signing twice. Lowest signature bytes wins,
        // deterministically, on every peer.
        match &self.declined {
            Some(current) if current.signature.to_bytes() <= incoming.signature.to_bytes() => {}
            _ => self.declined = Some(incoming),
        }
    }

    fn absorb_seat(&mut self, incoming: SignedAcceptance) {
        match &self.seated {
            None => self.seated = Some(incoming),
            Some(current) => {
                if incoming.wins_over(current) {
                    self.seated = Some(incoming);
                }
            }
        }
    }

    /// Drop what the merged contents cannot justify keeping: every offer once
    /// the seat is filled, and the lowest-keyed [`MAX_JOIN_OFFERS`] otherwise.
    /// A pure function of the contents, so it is idempotent and identical on
    /// every peer.
    pub(super) fn prune(&mut self) {
        if self.seated.is_some() {
            self.offers.clear();
            // A seat that got filled outranks a refusal: if the creator
            // countersigned somebody, the game is on, and a stale decline must
            // not be left implying otherwise.
            self.declined = None;
            return;
        }
        if self.declined.is_some() {
            // Refused: nobody is going to take this seat, so the offers are
            // dead weight.
            self.offers.clear();
            return;
        }
        while self.offers.len() > MAX_JOIN_OFFERS {
            let last = match self.offers.keys().next_back() {
                Some(k) => *k,
                None => break,
            };
            self.offers.remove(&last);
        }
    }
}

impl ComposableState for OpponentSlotV1 {
    type ParentState = ChessGameStateV1;
    /// Per offer, its author *and* a fingerprint of the offer itself, plus a
    /// fingerprint of the seat. Both digests are over signatures, which cover
    /// every field of what they sign.
    ///
    /// Summarising an offer by its author alone is not enough, and the test
    /// `two_offers_from_one_key_still_exchange` fails without this: one
    /// challenger who signs two offers leaves two peers reporting identical
    /// summaries, so neither ships, and the creator then sees a different offer
    /// depending on which peer it asks — meaning which one it can countersign
    /// turns on where it happens to be looking.
    type Summary = (Vec<([u8; 32], u64)>, Option<u64>, Option<u64>);
    type Delta = OpponentDelta;
    type Parameters = ChessGameParametersV1;

    fn verify(&self, parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        for offer in self.offers.values() {
            offer.verify(params)?;
            offer.check_invitation(parent)?;
        }
        if let Some(seat) = &self.seated {
            seat.verify(params)?;
            seat.join.check_invitation(parent)?;
        }
        if let Some(decline) = &self.declined {
            decline.verify(params, parent.setup.get().and_then(|s| s.setup.challenged))?;
        }
        Ok(())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        (
            self.offers
                .iter()
                .map(|(key, join)| (*key, signature_digest(&join.signature)))
                .collect(),
            self.seated.as_ref().map(|a| signature_digest(&a.signature)),
            self.declined
                .as_ref()
                .map(|d| signature_digest(&d.signature)),
        )
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let (their_offers, their_seat, their_decline) = old_summary;
        let theirs: BTreeMap<[u8; 32], u64> = their_offers.iter().copied().collect();
        let offers: Vec<SignedJoin> = self
            .offers
            .iter()
            .filter(|(key, join)| theirs.get(*key) != Some(&signature_digest(&join.signature)))
            .map(|(_, join)| join.clone())
            .collect();
        // Ship our seat whenever theirs differs — they may be holding the loser
        // of a double-acceptance and need ours to converge.
        let seated = match (&self.seated, their_seat) {
            (Some(mine), Some(theirs)) if signature_digest(&mine.signature) == *theirs => None,
            (mine, _) => mine.clone(),
        };
        let declined = match (&self.declined, their_decline) {
            (Some(mine), Some(theirs)) if signature_digest(&mine.signature) == *theirs => None,
            (mine, _) => mine.clone(),
        };
        if offers.is_empty() && seated.is_none() && declined.is_none() {
            None
        } else {
            Some(OpponentDelta {
                offers,
                seated,
                declined,
            })
        }
    }

    fn apply_delta(
        &mut self,
        parent: &Self::ParentState,
        params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        let Some(delta) = delta else { return Ok(()) };
        for offer in &delta.offers {
            offer.verify(params)?;
            offer.check_invitation(parent)?;
            self.absorb_offer(offer.clone());
        }
        if let Some(seat) = &delta.seated {
            seat.verify(params)?;
            seat.join.check_invitation(parent)?;
            self.absorb_seat(seat.clone());
        }
        if let Some(decline) = &delta.declined {
            decline.verify(params, parent.setup.get().and_then(|s| s.setup.challenged))?;
            self.absorb_decline(decline.clone());
        }
        Ok(())
    }
}

/// Offers and, once it exists, the countersigned seat.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpponentDelta {
    #[serde(default)]
    pub offers: Vec<SignedJoin>,
    #[serde(default)]
    pub seated: Option<SignedAcceptance>,
    #[serde(default)]
    pub declined: Option<SignedDecline>,
}

impl OpponentDelta {
    /// A delta carrying a single offer — what a challenger publishes.
    pub fn offer(join: SignedJoin) -> OpponentDelta {
        OpponentDelta {
            offers: vec![join],
            seated: None,
            declined: None,
        }
    }

    /// A delta carrying the seat — what the creator publishes to start play.
    pub fn seat(acceptance: SignedAcceptance) -> OpponentDelta {
        OpponentDelta {
            offers: Vec::new(),
            seated: Some(acceptance),
            declined: None,
        }
    }

    /// A delta refusing a direct challenge.
    pub fn decline(decline: SignedDecline) -> OpponentDelta {
        OpponentDelta {
            offers: Vec::new(),
            seated: None,
            declined: Some(decline),
        }
    }
}
