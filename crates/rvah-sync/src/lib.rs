//! Verified, transport-neutral snapshots of encrypted RVAH profiles.
//!
//! Veilid, a test double, or another untrusted record store may carry these
//! bytes. This crate deliberately knows nothing about profile keys or provider
//! credentials: it splits already-encrypted profile bundles and authenticates
//! their ordering with a small head record and a SHA-256 digest.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SNAPSHOT_FORMAT: &str = "rvah-profile-dht-v1";
pub const PROFILE_MAX_BYTES: usize = 512 * 1024;
pub const CHUNK_BYTES: usize = 24 * 1024;
pub const MAX_CHUNKS: usize = PROFILE_MAX_BYTES.div_ceil(CHUNK_BYTES);
pub const DHT_SUBKEY_COUNT: u32 = (MAX_CHUNKS + 1) as u32;
const MAX_HEAD_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotHead {
    pub format: String,
    pub epoch: u64,
    pub ciphertext_sha256: String,
    pub chunk_count: u16,
    pub byte_length: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedSnapshot {
    head: SnapshotHead,
    head_bytes: Vec<u8>,
    chunks: Vec<Vec<u8>>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("snapshot epoch must be greater than zero")]
    InvalidEpoch,
    #[error("encrypted profile is empty or exceeds 512 KiB")]
    InvalidBundleSize,
    #[error("snapshot head is malformed or too large")]
    InvalidHead,
    #[error("unsupported snapshot format")]
    UnsupportedFormat,
    #[error("snapshot epoch {actual} is older than required epoch {minimum}")]
    StaleEpoch { actual: u64, minimum: u64 },
    #[error("snapshot is incomplete or has an invalid chunk layout")]
    InvalidChunks,
    #[error("snapshot ciphertext digest does not match its signed head")]
    DigestMismatch,
}

impl PreparedSnapshot {
    pub fn new(epoch: u64, encrypted_profile: &[u8]) -> Result<Self, SnapshotError> {
        if epoch == 0 {
            return Err(SnapshotError::InvalidEpoch);
        }
        validate_bundle_size(encrypted_profile)?;
        let chunks: Vec<Vec<u8>> = encrypted_profile
            .chunks(CHUNK_BYTES)
            .map(ToOwned::to_owned)
            .collect();
        let head = SnapshotHead {
            format: SNAPSHOT_FORMAT.into(),
            epoch,
            ciphertext_sha256: digest_hex(encrypted_profile),
            chunk_count: chunks.len() as u16,
            byte_length: encrypted_profile.len() as u32,
        };
        let head_bytes = serde_json::to_vec(&head).map_err(|_| SnapshotError::InvalidHead)?;
        if head_bytes.len() > MAX_HEAD_BYTES {
            return Err(SnapshotError::InvalidHead);
        }
        Ok(Self {
            head,
            head_bytes,
            chunks,
        })
    }

    pub fn head(&self) -> &SnapshotHead {
        &self.head
    }

    pub fn head_bytes(&self) -> &[u8] {
        &self.head_bytes
    }

    pub fn chunks(&self) -> &[Vec<u8>] {
        &self.chunks
    }
}

pub fn parse_head(bytes: &[u8]) -> Result<SnapshotHead, SnapshotError> {
    if bytes.is_empty() || bytes.len() > MAX_HEAD_BYTES {
        return Err(SnapshotError::InvalidHead);
    }
    let head: SnapshotHead =
        serde_json::from_slice(bytes).map_err(|_| SnapshotError::InvalidHead)?;
    validate_head(&head)?;
    Ok(head)
}

pub fn assemble(
    head_bytes: &[u8],
    chunks: &[Vec<u8>],
    minimum_epoch: u64,
) -> Result<Vec<u8>, SnapshotError> {
    let head = parse_head(head_bytes)?;
    if head.epoch < minimum_epoch {
        return Err(SnapshotError::StaleEpoch {
            actual: head.epoch,
            minimum: minimum_epoch,
        });
    }
    if chunks.len() != usize::from(head.chunk_count) {
        return Err(SnapshotError::InvalidChunks);
    }
    for (index, chunk) in chunks.iter().enumerate() {
        let final_chunk = index + 1 == chunks.len();
        if chunk.is_empty()
            || chunk.len() > CHUNK_BYTES
            || (!final_chunk && chunk.len() != CHUNK_BYTES)
        {
            return Err(SnapshotError::InvalidChunks);
        }
    }
    let mut encrypted_profile = Vec::with_capacity(head.byte_length as usize);
    for chunk in chunks {
        encrypted_profile.extend_from_slice(chunk);
    }
    verify(&head, &encrypted_profile)?;
    Ok(encrypted_profile)
}

pub fn verify(head: &SnapshotHead, encrypted_profile: &[u8]) -> Result<(), SnapshotError> {
    validate_head(head)?;
    validate_bundle_size(encrypted_profile)?;
    if encrypted_profile.len() != head.byte_length as usize {
        return Err(SnapshotError::InvalidChunks);
    }
    let expected_chunks = encrypted_profile.len().div_ceil(CHUNK_BYTES);
    if expected_chunks != usize::from(head.chunk_count) {
        return Err(SnapshotError::InvalidChunks);
    }
    if digest_hex(encrypted_profile) != head.ciphertext_sha256 {
        return Err(SnapshotError::DigestMismatch);
    }
    Ok(())
}

fn validate_head(head: &SnapshotHead) -> Result<(), SnapshotError> {
    if head.format != SNAPSHOT_FORMAT {
        return Err(SnapshotError::UnsupportedFormat);
    }
    if head.epoch == 0 {
        return Err(SnapshotError::InvalidEpoch);
    }
    if head.chunk_count == 0 || usize::from(head.chunk_count) > MAX_CHUNKS {
        return Err(SnapshotError::InvalidChunks);
    }
    if head.byte_length == 0 || head.byte_length as usize > PROFILE_MAX_BYTES {
        return Err(SnapshotError::InvalidBundleSize);
    }
    if head.ciphertext_sha256.len() != 64
        || !head
            .ciphertext_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SnapshotError::InvalidHead);
    }
    Ok(())
}

fn validate_bundle_size(bytes: &[u8]) -> Result<(), SnapshotError> {
    if bytes.is_empty() || bytes.len() > PROFILE_MAX_BYTES {
        return Err(SnapshotError::InvalidBundleSize);
    }
    Ok(())
}

fn digest_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_roundtrip_preserves_ciphertext() {
        let bundle = vec![0x5a; CHUNK_BYTES * 2 + 17];
        let snapshot = PreparedSnapshot::new(7, &bundle).unwrap();
        assert_eq!(snapshot.head().epoch, 7);
        assert_eq!(snapshot.chunks().len(), 3);
        assert_eq!(
            assemble(snapshot.head_bytes(), snapshot.chunks(), 7).unwrap(),
            bundle
        );
    }

    #[test]
    fn incomplete_or_reordered_chunks_are_rejected() {
        let bundle: Vec<u8> = (0..CHUNK_BYTES * 2 + 3)
            .map(|index| (index % 251) as u8)
            .collect();
        let snapshot = PreparedSnapshot::new(2, &bundle).unwrap();

        let incomplete = &snapshot.chunks()[..2];
        assert_eq!(
            assemble(snapshot.head_bytes(), incomplete, 0),
            Err(SnapshotError::InvalidChunks)
        );

        let mut reordered = snapshot.chunks().to_vec();
        reordered.swap(0, 1);
        assert_eq!(
            assemble(snapshot.head_bytes(), &reordered, 0),
            Err(SnapshotError::DigestMismatch)
        );
    }

    #[test]
    fn stale_epoch_and_modified_ciphertext_are_rejected() {
        let bundle = vec![0x31; 256];
        let snapshot = PreparedSnapshot::new(4, &bundle).unwrap();
        assert_eq!(
            assemble(snapshot.head_bytes(), snapshot.chunks(), 5),
            Err(SnapshotError::StaleEpoch {
                actual: 4,
                minimum: 5
            })
        );

        let mut changed = snapshot.chunks().to_vec();
        changed[0][0] ^= 1;
        assert_eq!(
            assemble(snapshot.head_bytes(), &changed, 0),
            Err(SnapshotError::DigestMismatch)
        );
    }
}
