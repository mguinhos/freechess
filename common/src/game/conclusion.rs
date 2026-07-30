//! Ways a game ends off the board: resignation, agreed draw, and timeout.
//!
//! Each is authorized by the party who can legitimately claim it, and only
//! within this one game — every signature covers the `game_id`, so a
//! resignation signed in one game is meaningless in any other, even between the
//! same two players.

use super::{
    ChessGameParametersV1, ChessGameStateV1, DrawReason, GameResult, WinReason, SIG_DOMAIN,
};
use crate::chess::Color;
use crate::identity::{verify_sig, GameId, PlayerId};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};

/// How far past the instant it became provable a timeout claim may be dated.
///
/// The deadline itself is computed from state (see
/// [`ChessGameStateV1::timeout_provable_at`]), so an honest client can always
/// name it exactly and needs no slack below it. This window only absorbs the
/// delay between a flag falling and the winner noticing, and bounds how stale a
/// claim may be — without it, `at` would be free above the deadline and would
/// land arbitrary finish times in certificates and archive ordering.
pub const TIMEOUT_CLAIM_WINDOW_MS: i64 = 60_000;

/// Structural ceiling on stored claims: each of the two players may hold one
/// slot per kind, and nothing else is admissible.
pub const MAX_CONCLUSIONS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConclusionKind {
    /// The signer gives up. Signed by the losing player themselves.
    Resignation,
    /// Both players agreed to a draw; carries the opponent's countersignature.
    DrawAgreement,
    /// The signer claims their opponent's clock ran out.
    TimeoutClaim,
}

/// A signed statement that the game is over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedConclusion {
    pub kind: ConclusionKind,
    /// The player making the claim.
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub claimant: VerifyingKey,
    /// Ply count the game had reached; binds the claim to a point in the game
    /// so an old signed draw offer cannot be replayed later.
    pub at_ply: u32,
    /// Unix milliseconds.
    pub at: i64,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
    /// For [`ConclusionKind::DrawAgreement`], the opponent's signature over the
    /// same bytes. `None` for the other kinds.
    #[serde(default, with = "opt_signature_serde")]
    pub countersignature: Option<Signature>,
    /// The countersigning opponent's key, present only for a draw agreement.
    #[serde(default, with = "opt_key_serde")]
    pub counterparty: Option<VerifyingKey>,
}

/// Serde for the optional signature/key fields.
mod opt_signature_serde {
    use ed25519_dalek::Signature;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(sig: &Option<Signature>, s: S) -> Result<S::Ok, S::Error> {
        match sig {
            Some(sig) => Some(sig.to_bytes().to_vec()).serialize(s),
            None => None::<Vec<u8>>.serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Signature>, D::Error> {
        let bytes = Option::<Vec<u8>>::deserialize(d)?;
        match bytes {
            None => Ok(None),
            Some(b) => {
                let arr: [u8; 64] = b
                    .as_slice()
                    .try_into()
                    .map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))?;
                Ok(Some(Signature::from_bytes(&arr)))
            }
        }
    }
}

mod opt_key_serde {
    use ed25519_dalek::VerifyingKey;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(key: &Option<VerifyingKey>, s: S) -> Result<S::Ok, S::Error> {
        match key {
            Some(k) => Some(k.to_bytes().to_vec()).serialize(s),
            None => None::<Vec<u8>>.serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<VerifyingKey>, D::Error> {
        let bytes = Option::<Vec<u8>>::deserialize(d)?;
        match bytes {
            None => Ok(None),
            Some(b) => {
                let arr: [u8; 32] = b
                    .as_slice()
                    .try_into()
                    .map_err(|_| serde::de::Error::custom("key must be 32 bytes"))?;
                Ok(Some(
                    VerifyingKey::from_bytes(&arr).map_err(serde::de::Error::custom)?,
                ))
            }
        }
    }
}

/// Stable one-byte encoding of a kind, used both in the signed bytes and as
/// part of a claim's slot.
fn kind_tag(kind: ConclusionKind) -> u8 {
    match kind {
        ConclusionKind::Resignation => 0,
        ConclusionKind::DrawAgreement => 1,
        ConclusionKind::TimeoutClaim => 2,
    }
}

fn conclusion_signing_bytes(
    game_id: &GameId,
    kind: ConclusionKind,
    at_ply: u32,
    at: i64,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(SIG_DOMAIN);
    buf.extend_from_slice(b"conclusion:");
    buf.extend_from_slice(&game_id.0);
    buf.push(kind_tag(kind));
    buf.extend_from_slice(&at_ply.to_le_bytes());
    buf.extend_from_slice(&at.to_le_bytes());
    buf
}

impl SignedConclusion {
    /// Resign the game. Only ever costs the signer.
    pub fn resign(key: &SigningKey, game_id: &GameId, at_ply: u32, at: i64) -> SignedConclusion {
        let bytes = conclusion_signing_bytes(game_id, ConclusionKind::Resignation, at_ply, at);
        SignedConclusion {
            kind: ConclusionKind::Resignation,
            claimant: key.verifying_key(),
            at_ply,
            at,
            signature: key.sign(&bytes),
            countersignature: None,
            counterparty: None,
        }
    }

    /// A draw both players signed. Requires both keys, which is what stops one
    /// player from unilaterally declaring a draw.
    pub fn draw_agreement(
        proposer: &SigningKey,
        accepter: &SigningKey,
        game_id: &GameId,
        at_ply: u32,
        at: i64,
    ) -> SignedConclusion {
        let bytes = conclusion_signing_bytes(game_id, ConclusionKind::DrawAgreement, at_ply, at);
        SignedConclusion {
            kind: ConclusionKind::DrawAgreement,
            claimant: proposer.verifying_key(),
            at_ply,
            at,
            signature: proposer.sign(&bytes),
            countersignature: Some(accepter.sign(&bytes)),
            counterparty: Some(accepter.verifying_key()),
        }
    }

    /// Claim the opponent's clock expired.
    ///
    /// `at` is not taken on trust: it must match the instant the claim became
    /// provable from the opponent's *own* attestations, which
    /// [`ChessGameStateV1::timeout_provable_at`] computes from state. A client
    /// should pass exactly that value.
    pub fn claim_timeout(
        key: &SigningKey,
        game_id: &GameId,
        at_ply: u32,
        at: i64,
    ) -> SignedConclusion {
        let bytes = conclusion_signing_bytes(game_id, ConclusionKind::TimeoutClaim, at_ply, at);
        SignedConclusion {
            kind: ConclusionKind::TimeoutClaim,
            claimant: key.verifying_key(),
            at_ply,
            at,
            signature: key.sign(&bytes),
            countersignature: None,
            counterparty: None,
        }
    }

    fn signing_bytes(&self, game_id: &GameId) -> Vec<u8> {
        conclusion_signing_bytes(game_id, self.kind, self.at_ply, self.at)
    }

    /// Deterministic total order for settling concurrent conclusions: the
    /// earliest claim wins, signature bytes break an exact tie.
    pub fn order_key(&self) -> (i64, [u8; 64]) {
        (self.at, self.signature.to_bytes())
    }

    pub fn claimant_id(&self) -> PlayerId {
        PlayerId::from(&self.claimant)
    }

    /// The slot this claim occupies: one per player per kind. Structural, so a
    /// player cannot flood the conclusion set to bury a real claim.
    fn slot(&self) -> (PlayerId, u8) {
        (self.claimant_id(), kind_tag(self.kind))
    }

    /// Whether this claim actually decides the game, as opposed to merely being
    /// authentic.
    ///
    /// The split matters. Authenticity ([`verify`](Self::verify)) is a property
    /// of the bytes and never changes, so it is safe to enforce when a delta
    /// arrives. Whether a *timeout* claim holds depends on evidence that can
    /// still be in flight — the loser's own attestations — and so can change as
    /// state converges. Rejecting on that basis would let a state that was valid
    /// when applied fail validation later; deciding effect here instead keeps
    /// every stored claim permanently valid while making the outcome a pure
    /// function of the merged state, which is what every peer agreeing requires.
    ///
    /// A claim that is stored but ineffective is inert: it cannot end the game,
    /// and because slots are per-player-per-kind it cannot displace a claim that
    /// does hold.
    pub fn is_effective(&self, parent: &ChessGameStateV1) -> bool {
        match self.kind {
            // Costs only the signer, so it needs no corroboration.
            ConclusionKind::Resignation => true,
            // Both players signed; that is the whole authority for a draw.
            ConclusionKind::DrawAgreement => true,
            ConclusionKind::TimeoutClaim => {
                let loser = match parent.color_of(&self.claimant) {
                    Some(winner) => winner.opposite(),
                    None => return false,
                };
                match parent.timeout_provable_at(loser, self.at_ply) {
                    Some(provable) => {
                        self.at >= provable
                            && self.at <= provable.saturating_add(TIMEOUT_CLAIM_WINDOW_MS)
                    }
                    None => false,
                }
            }
        }
    }

    /// Validate the claim as a signed statement: that it is authentic, and that
    /// its author is entitled to make a statement of this kind about this game.
    ///
    /// Everything checked here is a property of the bytes and of facts that
    /// cannot be retracted, so a claim that passes today passes forever. Whether
    /// the claim *decides* the game is [`is_effective`](Self::is_effective).
    pub fn verify(
        &self,
        parent: &ChessGameStateV1,
        params: &ChessGameParametersV1,
    ) -> Result<(), String> {
        let (white, black) = parent
            .player_keys()
            .ok_or_else(|| "a game with no opponent cannot be concluded".to_string())?;

        // The claimant must be one of this game's two players — not merely
        // "some valid key". This is what keeps a third party from resigning
        // someone else's game.
        if self.claimant != white && self.claimant != black {
            return Err(
                "conclusion signed by someone who is not a player in this game".to_string(),
            );
        }
        if self.at <= 0 {
            return Err("conclusion timestamp must be positive".to_string());
        }
        verify_sig(
            &self.claimant,
            &self.signing_bytes(&params.game_id),
            &self.signature,
            "conclusion",
        )?;

        match self.kind {
            ConclusionKind::Resignation => Ok(()),

            ConclusionKind::DrawAgreement => {
                let counter_sig = self
                    .countersignature
                    .ok_or_else(|| "draw agreement lacks a countersignature".to_string())?;
                let counter_key = self
                    .counterparty
                    .ok_or_else(|| "draw agreement lacks a counterparty key".to_string())?;
                // The countersigner must be the *other* player: two signatures
                // from the same key are not an agreement.
                let expected_counter = if self.claimant == white { black } else { white };
                if counter_key != expected_counter {
                    return Err("draw countersigned by the wrong key".to_string());
                }
                verify_sig(
                    &counter_key,
                    &self.signing_bytes(&params.game_id),
                    &counter_sig,
                    "draw countersignature",
                )
            }

            // Nothing further to check on the bytes. Whether the clocks bear the
            // claim out is decided by `is_effective`, because it turns on the
            // loser's attestations, which may still be arriving.
            ConclusionKind::TimeoutClaim => Ok(()),
        }
    }

    /// The result this conclusion implies.
    pub fn result(&self, parent: &ChessGameStateV1) -> Option<GameResult> {
        let color = parent.color_of(&self.claimant)?;
        Some(match self.kind {
            // Resigning hands the win to the *other* side.
            ConclusionKind::Resignation => match color {
                Color::White => GameResult::BlackWins(WinReason::Resignation),
                Color::Black => GameResult::WhiteWins(WinReason::Resignation),
            },
            ConclusionKind::DrawAgreement => GameResult::Draw(DrawReason::Agreement),
            // A timeout claim is made by the winner.
            ConclusionKind::TimeoutClaim => match color {
                Color::White => GameResult::WhiteWins(WinReason::Timeout),
                Color::Black => GameResult::BlackWins(WinReason::Timeout),
            },
        })
    }
}

/// Every claim made about how this game ended. Empty while the game is live.
///
/// # Why a set rather than one slot
///
/// A single slot has to pick a winner the moment a claim arrives, and the pick
/// has to be a function of the claims alone — no parent state — or peers that
/// merged in different orders would keep different ones. That is fine when every
/// claim is unconditionally decisive, but a timeout claim is not: it holds only
/// if the loser's attestations bear it out, and those may still be in flight.
/// With one slot, a claim that turns out to be inert still occupies it, and can
/// bury the resignation or agreed draw that would have ended the game — so a
/// player could hang a game permanently by filing an early bogus timeout.
///
/// Keeping the set and choosing the effective claim at read time removes that:
/// merging is a union (commutative, associative, idempotent), and the outcome is
/// a pure function of the merged state, so every peer agrees without any claim
/// having to be discarded.
///
/// The set is structurally bounded rather than capped by an eviction rule: one
/// slot per player per kind, at most [`MAX_CONCLUSIONS`] in total. There is no
/// truncation for an attacker to aim at.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConclusionV1 {
    /// Sorted by [`SignedConclusion::order_key`], so the encoding is
    /// deterministic for a given set of claims.
    pub claims: Vec<SignedConclusion>,
}

impl ConclusionV1 {
    /// A set holding one claim, which is what a client publishing a resignation
    /// or a timeout builds.
    pub fn single(claim: SignedConclusion) -> ConclusionV1 {
        ConclusionV1 {
            claims: vec![claim],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }

    /// The claim that decides the game: the earliest one whose evidence holds.
    pub fn effective(&self, parent: &ChessGameStateV1) -> Option<&SignedConclusion> {
        self.claims.iter().find(|c| c.is_effective(parent))
    }

    /// The result implied by the deciding claim, if any.
    pub(super) fn result_with(&self, parent: &ChessGameStateV1) -> Option<GameResult> {
        self.effective(parent)?.result(parent)
    }

    /// Merge one claim in. Same-slot conflicts are settled by the total order,
    /// never by arrival order.
    fn absorb(&mut self, incoming: SignedConclusion) {
        match self.claims.iter_mut().find(|c| c.slot() == incoming.slot()) {
            Some(existing) => {
                if incoming.order_key() < existing.order_key() {
                    *existing = incoming;
                }
            }
            None => self.claims.push(incoming),
        }
        self.claims.sort_by_key(|c| c.order_key());
    }
}

impl ComposableState for ConclusionV1 {
    type ParentState = ChessGameStateV1;
    /// One signature per claim, which distinguishes exactly as much as the
    /// merge does. A coarser summary (this used to be a bare `bool`) makes two
    /// peers holding *different* claims both report "I have one" and neither
    /// ship a delta, so the total order that settles them is never reached and
    /// the divergence is permanent.
    type Summary = Vec<Vec<u8>>;
    type Delta = Vec<SignedConclusion>;
    type Parameters = ChessGameParametersV1;

    fn verify(&self, parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        if self.claims.len() > MAX_CONCLUSIONS {
            return Err(format!(
                "{} conclusions recorded, over the {MAX_CONCLUSIONS} a two-player game admits",
                self.claims.len()
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for claim in &self.claims {
            claim.verify(parent, params)?;
            if !seen.insert(claim.slot()) {
                return Err("two conclusions of the same kind from the same player".to_string());
            }
        }
        Ok(())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        self.claims
            .iter()
            .map(|c| c.signature.to_bytes().to_vec())
            .collect()
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let theirs: std::collections::BTreeSet<&[u8]> =
            old_summary.iter().map(|s| s.as_slice()).collect();
        let missing: Vec<SignedConclusion> = self
            .claims
            .iter()
            .filter(|c| !theirs.contains(c.signature.to_bytes().as_slice()))
            .cloned()
            .collect();
        if missing.is_empty() {
            None
        } else {
            Some(missing)
        }
    }

    fn apply_delta(
        &mut self,
        parent: &Self::ParentState,
        params: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        if let Some(incoming) = delta {
            for claim in incoming {
                claim.verify(parent, params)?;
                self.absorb(claim.clone());
            }
        }
        Ok(())
    }
}
