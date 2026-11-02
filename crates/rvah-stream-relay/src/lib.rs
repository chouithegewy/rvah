//! Bounded, memory-only media frames for a private-route transport.
//!
//! This crate is deliberately not a DHT video store. A trusted proxy reads an
//! authorized upstream segment, encrypts it into independently authenticated
//! frames, and retains only a bounded ciphertext window for retransmission.
//! Once every required client returns a receipt authenticated by the session
//! key, the relay drops that segment. Strict TTL and capacity eviction provide
//! the fallback when a client disappears.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, Instant};

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hmac::{Hmac, Mac};
use rand::TryRng;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

pub const FRAME_FORMAT: &str = "rvah-stream-frame-v1";
pub const RECEIPT_FORMAT: &str = "rvah-stream-receipt-v1";
pub const FRAME_PLAINTEXT_BYTES: usize = 48 * 1024;
pub const MAX_SEGMENT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CLIENT_ID_BYTES: usize = 128;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 16];
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .expect("operating system random number generator failed");
        Self(hex::encode(bytes))
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, RelayError> {
        let value = value.into();
        if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(RelayError::InvalidSession);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// A per-session secret enrolled out-of-band with authorized clients.
pub struct SessionKey([u8; 32]);

impl SessionKey {
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .expect("operating system random number generator failed");
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl Drop for SessionKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionKey([REDACTED])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedFrame {
    pub format: String,
    pub session_id: SessionId,
    pub segment: u64,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub segment_byte_length: u64,
    pub segment_sha256: String,
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub format: String,
    pub session_id: SessionId,
    pub segment: u64,
    pub byte_length: u64,
    pub plaintext_sha256: String,
    pub mac: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedSegment {
    pub bytes: Vec<u8>,
    pub receipt: DeliveryReceipt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentRef {
    pub session_id: SessionId,
    pub segment: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestOutcome {
    pub segment: SegmentRef,
    pub evicted: Vec<SegmentRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayLimits {
    pub max_buffer_bytes: usize,
    pub max_segments: usize,
    pub segment_ttl: Duration,
}

impl Default for RelayLimits {
    fn default() -> Self {
        Self {
            max_buffer_bytes: 64 * 1024 * 1024,
            max_segments: 32,
            segment_ttl: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RelayError {
    #[error("invalid stream session id")]
    InvalidSession,
    #[error("a stream session requires at least one valid client id")]
    InvalidClients,
    #[error("relay limits must be nonzero and hold at least one encrypted frame")]
    InvalidLimits,
    #[error("stream segment is empty or exceeds the 16 MiB segment limit")]
    InvalidSegment,
    #[error("stream session was not registered")]
    UnknownSession,
    #[error("stream segment already exists or its sequence is not increasing")]
    InvalidSequence,
    #[error("encrypted stream frames are incomplete or inconsistent")]
    InvalidFrames,
    #[error("stream frame or receipt authentication failed")]
    Authentication,
    #[error("client is not authorized for this stream session")]
    UnknownClient,
    #[error("stream segment is no longer in the relay buffer")]
    UnknownSegment,
}

struct Session {
    key: SessionKey,
    clients: BTreeSet<String>,
    last_segment: Option<u64>,
}

struct BufferedSegment {
    reference: SegmentRef,
    frames: Vec<EncryptedFrame>,
    byte_cost: usize,
    byte_length: u64,
    digest: String,
    pending_clients: BTreeSet<String>,
    expires_at: Instant,
}

/// In-memory retransmit window. It never opens files or writes segment data.
pub struct RelayBuffer {
    limits: RelayLimits,
    sessions: HashMap<SessionId, Session>,
    segments: VecDeque<BufferedSegment>,
    buffered_bytes: usize,
}

impl RelayBuffer {
    pub fn new(limits: RelayLimits) -> Result<Self, RelayError> {
        if limits.max_buffer_bytes < FRAME_PLAINTEXT_BYTES + 16
            || limits.max_segments == 0
            || limits.segment_ttl.is_zero()
        {
            return Err(RelayError::InvalidLimits);
        }
        Ok(Self {
            limits,
            sessions: HashMap::new(),
            segments: VecDeque::new(),
            buffered_bytes: 0,
        })
    }

    pub fn register_session(
        &mut self,
        session_id: SessionId,
        key: SessionKey,
        clients: impl IntoIterator<Item = String>,
    ) -> Result<(), RelayError> {
        let clients: Vec<String> = clients
            .into_iter()
            .map(|client| client.trim().to_owned())
            .collect();
        if clients.is_empty()
            || clients
                .iter()
                .any(|client| client.is_empty() || client.len() > MAX_CLIENT_ID_BYTES)
        {
            return Err(RelayError::InvalidClients);
        }
        let unique_clients: BTreeSet<String> = clients.iter().cloned().collect();
        if unique_clients.len() != clients.len() {
            return Err(RelayError::InvalidClients);
        }
        let clients = unique_clients;
        self.remove_session(&session_id);
        self.sessions.insert(
            session_id,
            Session {
                key,
                clients,
                last_segment: None,
            },
        );
        Ok(())
    }

    pub fn remove_session(&mut self, session_id: &SessionId) {
        self.sessions.remove(session_id);
        let mut retained = VecDeque::with_capacity(self.segments.len());
        while let Some(segment) = self.segments.pop_front() {
            if &segment.reference.session_id == session_id {
                self.buffered_bytes = self.buffered_bytes.saturating_sub(segment.byte_cost);
            } else {
                retained.push_back(segment);
            }
        }
        self.segments = retained;
    }

    pub fn ingest(
        &mut self,
        session_id: &SessionId,
        segment: u64,
        plaintext: &[u8],
        now: Instant,
    ) -> Result<IngestOutcome, RelayError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(RelayError::UnknownSession)?;
        if session
            .last_segment
            .is_some_and(|previous| segment <= previous)
        {
            return Err(RelayError::InvalidSequence);
        }
        let frames = encrypt_segment(session_id, segment, plaintext, &session.key)?;
        let byte_cost = frames.iter().map(|frame| frame.ciphertext.len()).sum();
        if byte_cost > self.limits.max_buffer_bytes {
            return Err(RelayError::InvalidSegment);
        }
        session.last_segment = Some(segment);
        let digest = digest_hex(plaintext);
        let reference = SegmentRef {
            session_id: session_id.clone(),
            segment,
        };
        self.segments.push_back(BufferedSegment {
            reference: reference.clone(),
            frames,
            byte_cost,
            byte_length: plaintext.len() as u64,
            digest,
            pending_clients: session.clients.clone(),
            expires_at: now + self.limits.segment_ttl,
        });
        self.buffered_bytes += byte_cost;
        let mut evicted = self.prune_expired(now);
        while self.buffered_bytes > self.limits.max_buffer_bytes
            || self.segments.len() > self.limits.max_segments
        {
            if let Some(removed) = self.pop_front() {
                evicted.push(removed);
            }
        }
        Ok(IngestOutcome {
            segment: reference,
            evicted,
        })
    }

    pub fn frames(&self, reference: &SegmentRef) -> Option<&[EncryptedFrame]> {
        self.segments
            .iter()
            .find(|segment| &segment.reference == reference)
            .map(|segment| segment.frames.as_slice())
    }

    /// Returns true only when the final required client ack discarded the data.
    pub fn acknowledge(
        &mut self,
        client_id: &str,
        receipt: &DeliveryReceipt,
    ) -> Result<bool, RelayError> {
        let session = self
            .sessions
            .get(&receipt.session_id)
            .ok_or(RelayError::UnknownSession)?;
        if !session.clients.contains(client_id) {
            return Err(RelayError::UnknownClient);
        }
        verify_receipt(receipt, &session.key)?;
        let index = self
            .segments
            .iter()
            .position(|segment| {
                segment.reference.session_id == receipt.session_id
                    && segment.reference.segment == receipt.segment
            })
            .ok_or(RelayError::UnknownSegment)?;
        let segment = &mut self.segments[index];
        if segment.byte_length != receipt.byte_length || segment.digest != receipt.plaintext_sha256
        {
            return Err(RelayError::Authentication);
        }
        segment.pending_clients.remove(client_id);
        if segment.pending_clients.is_empty() {
            let removed = self.segments.remove(index).expect("known segment index");
            self.buffered_bytes = self.buffered_bytes.saturating_sub(removed.byte_cost);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn prune_expired(&mut self, now: Instant) -> Vec<SegmentRef> {
        let mut expired = Vec::new();
        let mut retained = VecDeque::with_capacity(self.segments.len());
        while let Some(segment) = self.segments.pop_front() {
            if segment.expires_at <= now {
                self.buffered_bytes = self.buffered_bytes.saturating_sub(segment.byte_cost);
                expired.push(segment.reference);
            } else {
                retained.push_back(segment);
            }
        }
        self.segments = retained;
        expired
    }

    pub fn buffered_bytes(&self) -> usize {
        self.buffered_bytes
    }

    pub fn buffered_segments(&self) -> usize {
        self.segments.len()
    }

    fn pop_front(&mut self) -> Option<SegmentRef> {
        self.segments.pop_front().map(|segment| {
            self.buffered_bytes = self.buffered_bytes.saturating_sub(segment.byte_cost);
            segment.reference
        })
    }
}

pub fn encrypt_segment(
    session_id: &SessionId,
    segment: u64,
    plaintext: &[u8],
    key: &SessionKey,
) -> Result<Vec<EncryptedFrame>, RelayError> {
    if plaintext.is_empty() || plaintext.len() > MAX_SEGMENT_BYTES {
        return Err(RelayError::InvalidSegment);
    }
    let digest = digest_hex(plaintext);
    let chunks = plaintext.len().div_ceil(FRAME_PLAINTEXT_BYTES);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key.0));
    plaintext
        .chunks(FRAME_PLAINTEXT_BYTES)
        .enumerate()
        .map(|(index, chunk)| {
            let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
            let aad = frame_aad(
                session_id,
                segment,
                index as u32,
                chunks as u32,
                plaintext.len() as u64,
                &digest,
            );
            let ciphertext = cipher
                .encrypt(
                    &nonce,
                    Payload {
                        msg: chunk,
                        aad: &aad,
                    },
                )
                .map_err(|_| RelayError::Authentication)?;
            Ok(EncryptedFrame {
                format: FRAME_FORMAT.into(),
                session_id: session_id.clone(),
                segment,
                chunk_index: index as u32,
                chunk_count: chunks as u32,
                segment_byte_length: plaintext.len() as u64,
                segment_sha256: digest.clone(),
                nonce: nonce.into(),
                ciphertext,
            })
        })
        .collect()
}

pub fn receive_segment(
    frames: &[EncryptedFrame],
    key: &SessionKey,
) -> Result<ReceivedSegment, RelayError> {
    let first = frames.first().ok_or(RelayError::InvalidFrames)?;
    let chunk_count = usize::try_from(first.chunk_count).map_err(|_| RelayError::InvalidFrames)?;
    if first.format != FRAME_FORMAT
        || chunk_count == 0
        || chunk_count != frames.len()
        || first.segment_byte_length == 0
        || first.segment_byte_length > MAX_SEGMENT_BYTES as u64
        || !valid_digest(&first.segment_sha256)
    {
        return Err(RelayError::InvalidFrames);
    }
    let mut ordered = BTreeMap::new();
    for frame in frames {
        if frame.format != FRAME_FORMAT
            || frame.session_id != first.session_id
            || frame.segment != first.segment
            || frame.chunk_count != first.chunk_count
            || frame.segment_byte_length != first.segment_byte_length
            || frame.segment_sha256 != first.segment_sha256
            || frame.chunk_index >= first.chunk_count
            || frame.ciphertext.len() <= 16
            || frame.ciphertext.len() > FRAME_PLAINTEXT_BYTES + 16
            || ordered.insert(frame.chunk_index, frame).is_some()
        {
            return Err(RelayError::InvalidFrames);
        }
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key.0));
    let mut plaintext = Vec::with_capacity(first.segment_byte_length as usize);
    for (expected, frame) in ordered.values().enumerate() {
        if frame.chunk_index as usize != expected {
            return Err(RelayError::InvalidFrames);
        }
        let aad = frame_aad(
            &frame.session_id,
            frame.segment,
            frame.chunk_index,
            frame.chunk_count,
            frame.segment_byte_length,
            &frame.segment_sha256,
        );
        let chunk = cipher
            .decrypt(
                Nonce::from_slice(&frame.nonce),
                Payload {
                    msg: &frame.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| RelayError::Authentication)?;
        plaintext.extend_from_slice(&chunk);
    }
    if plaintext.len() as u64 != first.segment_byte_length
        || digest_hex(&plaintext) != first.segment_sha256
    {
        plaintext.zeroize();
        return Err(RelayError::Authentication);
    }
    let receipt = make_receipt(
        &first.session_id,
        first.segment,
        first.segment_byte_length,
        &first.segment_sha256,
        key,
    );
    Ok(ReceivedSegment {
        bytes: plaintext,
        receipt,
    })
}

pub fn verify_receipt(receipt: &DeliveryReceipt, key: &SessionKey) -> Result<(), RelayError> {
    if receipt.format != RECEIPT_FORMAT || !valid_digest(&receipt.plaintext_sha256) {
        return Err(RelayError::Authentication);
    }
    let supplied = hex::decode(&receipt.mac).map_err(|_| RelayError::Authentication)?;
    let ack_key = Zeroizing::new(ack_key(key));
    let mut verifier =
        <HmacSha256 as Mac>::new_from_slice(ack_key.as_ref()).expect("HMAC accepts any key");
    verifier.update(&receipt_body(
        &receipt.session_id,
        receipt.segment,
        receipt.byte_length,
        &receipt.plaintext_sha256,
    ));
    verifier
        .verify_slice(&supplied)
        .map_err(|_| RelayError::Authentication)?;
    Ok(())
}

fn make_receipt(
    session_id: &SessionId,
    segment: u64,
    byte_length: u64,
    digest: &str,
    key: &SessionKey,
) -> DeliveryReceipt {
    DeliveryReceipt {
        format: RECEIPT_FORMAT.into(),
        session_id: session_id.clone(),
        segment,
        byte_length,
        plaintext_sha256: digest.into(),
        mac: receipt_mac(session_id, segment, byte_length, digest, key),
    }
}

fn receipt_mac(
    session_id: &SessionId,
    segment: u64,
    byte_length: u64,
    digest: &str,
    key: &SessionKey,
) -> String {
    let ack_key = Zeroizing::new(ack_key(key));
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(ack_key.as_ref()).expect("HMAC accepts any key");
    mac.update(&receipt_body(session_id, segment, byte_length, digest));
    hex::encode(mac.finalize().into_bytes())
}

fn ack_key(key: &SessionKey) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"rvah-stream-receipt-key-v1\0");
    hasher.update(key.0);
    hasher.finalize().into()
}

fn receipt_body(session_id: &SessionId, segment: u64, byte_length: u64, digest: &str) -> Vec<u8> {
    format!(
        "{RECEIPT_FORMAT}\n{}\n{segment}\n{byte_length}\n{digest}",
        session_id.as_str()
    )
    .into_bytes()
}

fn frame_aad(
    session_id: &SessionId,
    segment: u64,
    chunk_index: u32,
    chunk_count: u32,
    byte_length: u64,
    digest: &str,
) -> Vec<u8> {
    format!(
        "{FRAME_FORMAT}\n{}\n{segment}\n{chunk_index}\n{chunk_count}\n{byte_length}\n{digest}",
        session_id.as_str()
    )
    .into_bytes()
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn digest_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(clients: &[&str], limits: RelayLimits) -> (RelayBuffer, SessionId, SessionKey) {
        let session_id = SessionId::parse("11".repeat(16)).unwrap();
        let relay_key = SessionKey::from_bytes([7; 32]);
        let client_key = SessionKey::from_bytes([7; 32]);
        let mut relay = RelayBuffer::new(limits).unwrap();
        relay
            .register_session(
                session_id.clone(),
                relay_key,
                clients.iter().map(|client| (*client).to_owned()),
            )
            .unwrap();
        (relay, session_id, client_key)
    }

    #[test]
    fn verified_client_receipts_discard_after_the_final_client() {
        let (mut relay, session_id, key) =
            setup(&["living-room", "tablet"], RelayLimits::default());
        let now = Instant::now();
        let bytes: Vec<u8> = (0..FRAME_PLAINTEXT_BYTES * 2 + 9)
            .map(|index| (index % 251) as u8)
            .collect();
        let ingested = relay.ingest(&session_id, 8, &bytes, now).unwrap();
        let received = receive_segment(relay.frames(&ingested.segment).unwrap(), &key).unwrap();
        assert_eq!(received.bytes, bytes);
        assert!(!relay.acknowledge("living-room", &received.receipt).unwrap());
        assert_eq!(relay.buffered_segments(), 1);
        assert!(relay.acknowledge("tablet", &received.receipt).unwrap());
        assert_eq!(relay.buffered_segments(), 0);
        assert_eq!(relay.buffered_bytes(), 0);
    }

    #[test]
    fn changed_frames_and_forged_receipts_are_rejected() {
        let (mut relay, session_id, key) = setup(&["client"], RelayLimits::default());
        let ingested = relay
            .ingest(&session_id, 1, b"authorized media bytes", Instant::now())
            .unwrap();
        let mut changed = relay.frames(&ingested.segment).unwrap().to_vec();
        changed[0].ciphertext[0] ^= 1;
        assert_eq!(
            receive_segment(&changed, &key),
            Err(RelayError::Authentication)
        );

        let mut receipt = receive_segment(relay.frames(&ingested.segment).unwrap(), &key)
            .unwrap()
            .receipt;
        receipt.byte_length += 1;
        assert_eq!(
            relay.acknowledge("client", &receipt),
            Err(RelayError::Authentication)
        );
        assert_eq!(relay.buffered_segments(), 1);
    }

    #[test]
    fn capacity_and_ttl_turn_over_the_ram_window() {
        let limits = RelayLimits {
            max_buffer_bytes: FRAME_PLAINTEXT_BYTES * 3,
            max_segments: 2,
            segment_ttl: Duration::from_secs(5),
        };
        let (mut relay, session_id, _) = setup(&["client"], limits);
        let now = Instant::now();
        let first = relay.ingest(&session_id, 1, &[1; 1024], now).unwrap();
        relay.ingest(&session_id, 2, &[2; 1024], now).unwrap();
        let third = relay.ingest(&session_id, 3, &[3; 1024], now).unwrap();
        assert_eq!(third.evicted, vec![first.segment]);
        assert_eq!(relay.buffered_segments(), 2);

        let expired = relay.prune_expired(now + Duration::from_secs(5));
        assert_eq!(expired.len(), 2);
        assert_eq!(relay.buffered_bytes(), 0);
    }

    #[test]
    fn invalid_client_cannot_be_silently_dropped_from_required_receipts() {
        let mut relay = RelayBuffer::new(RelayLimits::default()).unwrap();
        assert_eq!(
            relay.register_session(
                SessionId::parse("22".repeat(16)).unwrap(),
                SessionKey::from_bytes([9; 32]),
                ["living-room".into(), "".into()],
            ),
            Err(RelayError::InvalidClients)
        );
    }
}
