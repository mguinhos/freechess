//! Metadata for the contract that hosts the web UI.
//!
//! The gateway serves a webapp from a contract's state at
//! `/v1/contract/web/{key}/`, expecting this layout:
//!
//! ```text
//! [metadata_len: u64 BE][metadata: CBOR][web_len: u64 BE][web: tar.xz]
//! ```
//!
//! The contract's *parameters* are the publisher's 32-byte verifying key, so
//! the contract key itself commits to who is allowed to publish. That is what
//! makes the URL stable and trustworthy: a peer serving the app cannot swap in
//! different HTML, because the state would fail the signature check against a
//! key baked into the address the user already has.

use ed25519_dalek::Signature;
use serde::{Deserialize, Serialize};

/// Bytes the publisher signs: the version followed by the archive.
///
/// Including the version stops an old, correctly-signed release from being
/// replayed over a newer one — the signature only authorises that archive *at
/// that version*.
pub fn signing_bytes(version: u32, webapp: &[u8]) -> Vec<u8> {
    let mut message = version.to_be_bytes().to_vec();
    message.extend_from_slice(webapp);
    message
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WebContainerMetadata {
    /// Monotonic release counter. Version 0 is reserved as "unset".
    pub version: u32,
    /// Publisher's signature over [`signing_bytes`].
    pub signature: Signature,
}

/// Assemble the full contract state from its parts.
pub fn encode_state(metadata_cbor: &[u8], webapp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + metadata_cbor.len() + 8 + webapp.len());
    out.extend_from_slice(&(metadata_cbor.len() as u64).to_be_bytes());
    out.extend_from_slice(metadata_cbor);
    out.extend_from_slice(&(webapp.len() as u64).to_be_bytes());
    out.extend_from_slice(webapp);
    out
}

/// Split contract state back into `(metadata_cbor, webapp)`.
pub fn decode_state(state: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let read_len = |bytes: &[u8], at: usize| -> Result<usize, String> {
        let end = at
            .checked_add(8)
            .ok_or_else(|| "length field overflows".to_string())?;
        let slice = bytes
            .get(at..end)
            .ok_or_else(|| "truncated length field".to_string())?;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(slice);
        Ok(u64::from_be_bytes(buf) as usize)
    };

    let metadata_len = read_len(state, 0)?;
    let metadata_start: usize = 8;
    let metadata_end = metadata_start
        .checked_add(metadata_len)
        .ok_or_else(|| "metadata length overflows".to_string())?;
    let metadata = state
        .get(metadata_start..metadata_end)
        .ok_or_else(|| "truncated metadata".to_string())?;

    let web_len = read_len(state, metadata_end)?;
    let web_start = metadata_end + 8;
    let web_end = web_start
        .checked_add(web_len)
        .ok_or_else(|| "web length overflows".to_string())?;
    let webapp = state
        .get(web_start..web_end)
        .ok_or_else(|| "truncated web archive".to_string())?;

    Ok((metadata, webapp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips() {
        let metadata = b"metadata bytes";
        let webapp = b"a pretend tar.xz archive";
        let state = encode_state(metadata, webapp);
        let (m, w) = decode_state(&state).expect("decodes");
        assert_eq!(m, metadata);
        assert_eq!(w, webapp);
    }

    #[test]
    fn truncated_state_is_an_error_not_a_panic() {
        // A malformed state arrives from an untrusted peer, so every failure
        // path has to be an error rather than an index-out-of-bounds.
        let full = encode_state(b"meta", b"web");
        for cut in 0..full.len() {
            assert!(decode_state(&full[..cut]).is_err(), "should reject {cut}");
        }
    }

    #[test]
    fn a_length_field_claiming_more_than_the_buffer_is_rejected() {
        let mut state = encode_state(b"meta", b"web");
        // Claim a metadata length far beyond the buffer.
        state[..8].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(decode_state(&state).is_err());
    }

    #[test]
    fn signing_bytes_are_version_prefixed() {
        let a = signing_bytes(1, b"same archive");
        let b = signing_bytes(2, b"same archive");
        assert_ne!(a, b, "a version bump must change the signed bytes");
        assert_eq!(&a[..4], &1u32.to_be_bytes());
    }
}
