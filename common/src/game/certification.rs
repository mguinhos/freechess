//! Where the two players exchange the signatures that make a game *rated*.
//!
//! # Why an exchange is needed at all
//!
//! A rating is only trustworthy because both players signed the same record of
//! the game: the result, and the ratings each held going in. One signature
//! would let the winner declare whatever inputs flattered them — beating a
//! 3000-rated opponent pays much better than beating a 1200-rated one — so
//! [`GameCertificate`] carries two, over identical bytes.
//!
//! "Identical" is the hard part, and it is why this module exists rather than
//! each client simply signing its own copy. Two of the signed fields cannot be
//! derived independently:
//!
//! * `finished_at` is a wall clock, and two machines never agree to the
//!   millisecond.
//! * `white_rating_before` / `black_rating_before` come from the lobby's
//!   leaderboard, which is live state that changes as other games finish.
//!
//! So this is a handshake, in the same shape as the one that seats an opponent:
//! one player publishes the exact draft they signed, and the other signs *those
//! bytes* — after checking the draft against what they can see themselves. The
//! second player is not being asked to trust the first; they are being asked to
//! agree, and they refuse if the draft says anything they disagree with. A
//! refusal simply leaves the game unrated, which is the right way to fail.
//!
//! # Convergence
//!
//! Both players may publish first, each with their own draft. The map is keyed
//! by signer, so both survive the merge, and a client that finds a rival draft
//! adopts whichever wins a total order over the signed bytes — the same choice
//! on every peer, from the same set, in any order. Once both entries carry
//! byte-identical drafts, any client at all can assemble the certificate:
//! that is what [`CertificationV1::certificate`] does, and it needs no
//! privileged writer.

use super::{ChessGameParametersV1, ChessGameStateV1};
use crate::certificate::{CertificateDraft, GameCertificate, MAX_CERTIFICATE_TIME_MS};
use crate::elo::{MAX_ELO, MIN_ELO};
use crate::identity::{signature_digest, verify_sig};
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One player's signature over a specific draft, published so the other can
/// countersign the same bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCertificateProposal {
    pub draft: CertificateDraft,
    #[serde(with = "crate::identity::verifying_key_serde")]
    pub signer: VerifyingKey,
    #[serde(with = "crate::identity::signature_serde")]
    pub signature: Signature,
}

impl SignedCertificateProposal {
    pub fn new(key: &SigningKey, draft: CertificateDraft) -> SignedCertificateProposal {
        let signature = draft.sign(key);
        SignedCertificateProposal {
            draft,
            signer: key.verifying_key(),
            signature,
        }
    }

    /// Whether this proposal signs the same record as `other`.
    pub fn agrees_with(&self, other: &SignedCertificateProposal) -> bool {
        self.draft == other.draft
    }

    /// Total order over the *record*, used to pick which of two rival drafts
    /// both players should converge on. Deterministic and independent of
    /// arrival order; the signer is not part of it, so both peers choose alike.
    pub fn draft_order(&self) -> Vec<u8> {
        self.draft.signing_bytes()
    }

    /// Check the signature, and that the draft describes *this* game as the
    /// contract itself sees it.
    ///
    /// What cannot be checked here is the pre-game ratings: they live in the
    /// lobby, and a contract cannot read another contract's state. Those rest
    /// entirely on the countersignature — a player who disagrees does not sign,
    /// and without both halves nothing is ever filed. The bounds below only
    /// keep a nonsensical value from being recorded at all.
    pub fn verify(
        &self,
        parent: &ChessGameStateV1,
        params: &ChessGameParametersV1,
    ) -> Result<(), String> {
        let (white, black) = parent
            .player_keys()
            .ok_or_else(|| "a game with no opponent cannot be certified".to_string())?;
        if self.signer != white && self.signer != black {
            return Err("only the two players may certify a game".to_string());
        }
        if self.draft.game_id != params.game_id {
            return Err("certificate draft is for a different game".to_string());
        }
        if self.draft.white != white || self.draft.black != black {
            return Err("certificate draft names different players".to_string());
        }

        let result = parent.result();
        if !result.is_over() {
            return Err("the game is not over".to_string());
        }
        if self.draft.result != result {
            return Err("certificate draft claims a different result".to_string());
        }
        if self.draft.moves != parent.moves.move_list() {
            return Err("certificate draft carries a different move list".to_string());
        }

        let setup = parent
            .setup
            .get()
            .ok_or_else(|| "the game has no setup".to_string())?;
        if self.draft.time_control != setup.setup.time_control {
            return Err("certificate draft claims a different time control".to_string());
        }

        // `finished_at` is the one field nobody can check exactly — it is when
        // a client noticed the game had ended. Bound it rather than pretend:
        // it cannot predate the game, and the ceiling is what stops a wild
        // value from choosing an absurd archive shard.
        if self.draft.finished_at < self.draft.started_at
            || self.draft.finished_at > MAX_CERTIFICATE_TIME_MS
        {
            return Err("certificate draft has an implausible finish time".to_string());
        }
        for rating in [
            self.draft.white_rating_before,
            self.draft.black_rating_before,
        ] {
            if !(MIN_ELO..=MAX_ELO).contains(&rating) {
                return Err("certificate draft has an out-of-range rating".to_string());
            }
        }

        verify_sig(
            &self.signer,
            &self.draft.signing_bytes(),
            &self.signature,
            "certificate proposal",
        )
    }
}

/// The two halves of a certificate, in transit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificationV1 {
    /// Keyed by signer, so each player has exactly one live proposal and the
    /// two survive the merge independently.
    #[serde(default)]
    pub proposals: BTreeMap<[u8; 32], SignedCertificateProposal>,
}

impl CertificationV1 {
    pub fn is_empty(&self) -> bool {
        self.proposals.is_empty()
    }

    pub fn proposal_by(&self, player: &VerifyingKey) -> Option<&SignedCertificateProposal> {
        self.proposals.get(player.as_bytes())
    }

    /// The finished certificate, once both players have signed the same record.
    ///
    /// Any client can call this — assembling is not a privilege, because the
    /// result is fully determined by the two signatures already in state.
    pub fn certificate(&self, parent: &ChessGameStateV1) -> Option<GameCertificate> {
        let (white, black) = parent.player_keys()?;
        let by_white = self.proposal_by(&white)?;
        let by_black = self.proposal_by(&black)?;
        if !by_white.agrees_with(by_black) {
            return None;
        }
        Some(
            by_white
                .draft
                .clone()
                .assemble(by_white.signature, by_black.signature),
        )
    }

    /// Whichever rival draft both players should settle on, by a total order
    /// over the signed bytes.
    pub fn winning_draft(&self) -> Option<&CertificateDraft> {
        self.proposals
            .values()
            .min_by(|a, b| a.draft_order().cmp(&b.draft_order()))
            .map(|p| &p.draft)
    }

    fn absorb(&mut self, incoming: SignedCertificateProposal) {
        // Rank by the *record* first, signature bytes only to break a tie. Two
        // consequences, both needed:
        //
        // * A player can move onto a lower-ordered draft — which is exactly
        //   what adopting the other side's proposal is. Keeping "whichever
        //   signature sorts lowest" instead, as this first did, pinned each
        //   player to their first draft and the two could never meet.
        // * The move is one-way, so the merge stays monotone: every peer walks
        //   down the same total order and stops at the same place, in any
        //   order and however many times it merges.
        let rank = |p: &SignedCertificateProposal| (p.draft_order(), p.signature.to_bytes());
        match self.proposals.get(incoming.signer.as_bytes()) {
            Some(current) if rank(current) <= rank(&incoming) => {}
            _ => {
                self.proposals.insert(*incoming.signer.as_bytes(), incoming);
            }
        }
    }

    /// Drop anything the merged state cannot justify: proposals from anyone who
    /// is not a player, and everything at all while the game is still running.
    pub(super) fn prune(&mut self, parent_keys: Option<(VerifyingKey, VerifyingKey)>, over: bool) {
        if !over {
            self.proposals.clear();
            return;
        }
        let Some((white, black)) = parent_keys else {
            self.proposals.clear();
            return;
        };
        self.proposals
            .retain(|key, _| key == white.as_bytes() || key == black.as_bytes());
    }
}

impl ComposableState for CertificationV1 {
    type ParentState = ChessGameStateV1;
    /// Per proposal, its signer and a fingerprint of the signature — which
    /// covers the whole draft. Summarising by signer alone would leave two
    /// peers holding different drafts from the same player unable to tell, and
    /// they would never converge on which one to countersign.
    type Summary = Vec<([u8; 32], u64)>;
    type Delta = Vec<SignedCertificateProposal>;
    type Parameters = ChessGameParametersV1;

    fn verify(&self, parent: &Self::ParentState, params: &Self::Parameters) -> Result<(), String> {
        for proposal in self.proposals.values() {
            proposal.verify(parent, params)?;
        }
        Ok(())
    }

    fn summarize(&self, _parent: &Self::ParentState, _params: &Self::Parameters) -> Self::Summary {
        self.proposals
            .iter()
            .map(|(signer, p)| (*signer, signature_digest(&p.signature)))
            .collect()
    }

    fn delta(
        &self,
        _parent: &Self::ParentState,
        _params: &Self::Parameters,
        old_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let theirs: BTreeMap<[u8; 32], u64> = old_summary.iter().copied().collect();
        let missing: Vec<SignedCertificateProposal> = self
            .proposals
            .iter()
            .filter(|(signer, p)| theirs.get(*signer) != Some(&signature_digest(&p.signature)))
            .map(|(_, p)| p.clone())
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
        let Some(proposals) = delta else {
            return Ok(());
        };
        for proposal in proposals {
            // Skip rather than fail the whole delta: a proposal that no longer
            // matches the game (a later move arrived, say) is stale, not an
            // attack, and rejecting the batch would block the valid ones.
            if proposal.verify(parent, params).is_ok() {
                self.absorb(proposal.clone());
            }
        }
        Ok(())
    }
}
