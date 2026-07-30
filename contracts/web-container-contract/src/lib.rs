//! The contract that hosts the FreeChess UI.
//!
//! Its parameters are the publisher's 32-byte verifying key, so the contract
//! key — and therefore the URL users bookmark — commits to who may publish
//! there. A peer hosting the app cannot serve altered HTML: the state would
//! fail the signature check against a key already baked into the address.
//!
//! Updates are monotonic in `version`, which is what stops an old but validly
//! signed release from being replayed over a newer one.

use chess_core::web_container::{decode_state, signing_bytes, WebContainerMetadata};
use ciborium::de::from_reader;
use ed25519_dalek::VerifyingKey;
use freenet_stdlib::prelude::*;

/// Generous ceiling on the CBOR metadata blob (version + 64-byte signature is
/// well under 100 bytes).
const MAX_METADATA_SIZE: usize = 1024;

/// Ceiling on the packaged webapp.
const MAX_WEB_SIZE: usize = 100 * 1024 * 1024;

pub struct WebContainerContract;

/// Parse and fully validate a state blob, returning its version.
fn validate_bytes(parameters: &[u8], state: &[u8]) -> Result<u32, ContractError> {
    if parameters.len() != 32 {
        return Err(ContractError::Other(format!(
            "parameters must be a 32-byte verifying key, got {} bytes",
            parameters.len()
        )));
    }
    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(parameters);
    let publisher = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| ContractError::Other(format!("invalid publisher key: {e}")))?;

    let (metadata_bytes, webapp) = decode_state(state).map_err(|e| ContractError::Deser(e))?;

    if metadata_bytes.len() > MAX_METADATA_SIZE {
        return Err(ContractError::Other(format!(
            "metadata is {} bytes, over the {MAX_METADATA_SIZE} limit",
            metadata_bytes.len()
        )));
    }
    if webapp.len() > MAX_WEB_SIZE {
        return Err(ContractError::Other(format!(
            "webapp is {} bytes, over the {MAX_WEB_SIZE} limit",
            webapp.len()
        )));
    }

    let metadata: WebContainerMetadata = from_reader(metadata_bytes)
        .map_err(|e| ContractError::Deser(format!("could not decode metadata: {e}")))?;

    // Version 0 is reserved for "unset", so a state claiming it is malformed
    // rather than merely old.
    if metadata.version == 0 {
        return Err(ContractError::InvalidState);
    }

    // `verify_strict` rejects the small-order public keys that plain `verify`
    // accepts — worth having when the key comes from contract parameters an
    // attacker chose.
    publisher
        .verify_strict(
            &signing_bytes(metadata.version, webapp),
            &metadata.signature,
        )
        .map_err(|e| ContractError::InvalidUpdateWithInfo {
            reason: format!("webapp signature does not verify: {e}"),
        })?;

    Ok(metadata.version)
}

#[contract]
impl ContractInterface for WebContainerContract {
    fn validate_state(
        parameters: Parameters<'static>,
        state: State<'static>,
        _related: RelatedContracts<'static>,
    ) -> Result<ValidateResult, ContractError> {
        if state.as_ref().is_empty() {
            return Ok(ValidateResult::Valid);
        }
        validate_bytes(parameters.as_ref(), state.as_ref()).map(|_| ValidateResult::Valid)
    }

    fn update_state(
        parameters: Parameters<'static>,
        state: State<'static>,
        data: Vec<UpdateData<'static>>,
    ) -> Result<UpdateModification<'static>, ContractError> {
        let current_version = if state.as_ref().is_empty() {
            0
        } else {
            validate_bytes(parameters.as_ref(), state.as_ref())?
        };

        let mut best = state.as_ref().to_vec();
        let mut best_version = current_version;

        for update in data {
            // A webapp is replaced wholesale, so only a full state is
            // meaningful here; there is nothing to merge incrementally.
            let candidate = match update {
                UpdateData::State(new_state) => new_state.as_ref().to_vec(),
                UpdateData::StateAndDelta { state, .. } => state.as_ref().to_vec(),
                _ => return Err(ContractError::InvalidUpdate),
            };
            let version = validate_bytes(parameters.as_ref(), &candidate)?;
            // Monotonic: a validly signed *older* release must not overwrite a
            // newer one, or a downgrade attack works with no forgery at all.
            if version > best_version {
                best_version = version;
                best = candidate;
            }
        }

        Ok(UpdateModification::valid(best.into()))
    }

    fn summarize_state(
        _parameters: Parameters<'static>,
        state: State<'static>,
    ) -> Result<StateSummary<'static>, ContractError> {
        // The version alone identifies a release, so it is the whole summary.
        let version = if state.as_ref().is_empty() {
            0u32
        } else {
            let (metadata_bytes, _) = decode_state(state.as_ref()).map_err(ContractError::Deser)?;
            let metadata: WebContainerMetadata =
                from_reader(metadata_bytes).map_err(|e| ContractError::Deser(e.to_string()))?;
            metadata.version
        };
        Ok(StateSummary::from(version.to_be_bytes().to_vec()))
    }

    fn get_state_delta(
        _parameters: Parameters<'static>,
        state: State<'static>,
        summary: StateSummary<'static>,
    ) -> Result<StateDelta<'static>, ContractError> {
        let their_version = match <[u8; 4]>::try_from(summary.as_ref()) {
            Ok(bytes) => u32::from_be_bytes(bytes),
            // An unreadable or empty summary means the peer has nothing.
            Err(_) => 0,
        };

        let ours = if state.as_ref().is_empty() {
            0
        } else {
            let (metadata_bytes, _) = decode_state(state.as_ref()).map_err(ContractError::Deser)?;
            let metadata: WebContainerMetadata =
                from_reader(metadata_bytes).map_err(|e| ContractError::Deser(e.to_string()))?;
            metadata.version
        };

        // There is no incremental form of a webapp, so bring them up to date
        // with the whole thing — or send nothing if they already have it.
        if ours > their_version {
            Ok(StateDelta::from(state.as_ref().to_vec()))
        } else {
            Ok(StateDelta::from(Vec::new()))
        }
    }
}
