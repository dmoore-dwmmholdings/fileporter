//! Fileporter protocol v1 framing and state validation.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const ALPN: &str = "fileporter/1";
pub const PROTOCOL_PREFACE: [u8; 13] = *b"FILEPORTER\0\0\x01";
pub const FRAME_HEADER_LEN: usize = 5;
pub const MAX_CONTROL_PAYLOAD: usize = 1024 * 1024;
pub const MAX_CHUNK_DATA: usize = 1024 * 1024;
pub const CHUNK_FIXED_LEN: usize = 16 + 16 + 8 + 4 + 32;
pub const MAX_CHUNK_PAYLOAD: usize = CHUNK_FIXED_LEN + MAX_CHUNK_DATA;

const CONTROL_KIND: u8 = 1;
const CHUNK_KIND: u8 = 2;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("truncated frame")]
    TruncatedFrame,
    #[error("frame payload of {actual} bytes exceeds {limit} byte limit")]
    OversizedFrame { actual: usize, limit: usize },
    #[error("unknown frame kind {0}")]
    UnknownFrameKind(u8),
    #[error("invalid control payload: {0}")]
    InvalidControl(String),
    #[error("invalid chunk payload")]
    InvalidChunk,
    #[error("chunk data length {actual} exceeds {limit} byte limit")]
    OversizedChunk { actual: usize, limit: usize },
    #[error("chunk hash mismatch")]
    ChunkHashMismatch,
    #[error("message {message} is invalid in session phase {phase}")]
    InvalidState {
        message: &'static str,
        phase: SessionPhase,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub batch_id: Uuid,
    pub entry_id: Uuid,
    pub offset: u64,
    pub data: Vec<u8>,
}

impl Chunk {
    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&self.data).as_bytes()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Control(ControlMessage),
    Chunk(Chunk),
}

/// Returns the full frame length when the header is available.
pub fn frame_len(input: &[u8]) -> Result<usize, ProtocolError> {
    if input.len() < FRAME_HEADER_LEN {
        return Err(ProtocolError::TruncatedFrame);
    }
    let declared =
        u32::from_be_bytes(input[1..5].try_into().expect("header length checked")) as usize;
    let limit = match input[0] {
        CONTROL_KIND => MAX_CONTROL_PAYLOAD,
        CHUNK_KIND => MAX_CHUNK_PAYLOAD,
        kind => return Err(ProtocolError::UnknownFrameKind(kind)),
    };
    if declared > limit {
        return Err(ProtocolError::OversizedFrame {
            actual: declared,
            limit,
        });
    }
    Ok(FRAME_HEADER_LEN + declared)
}

pub fn encode_control(message: &ControlMessage) -> Result<Vec<u8>, ProtocolError> {
    let payload = serde_json::to_vec(message)
        .map_err(|error| ProtocolError::InvalidControl(error.to_string()))?;
    if payload.len() > MAX_CONTROL_PAYLOAD {
        return Err(ProtocolError::OversizedFrame {
            actual: payload.len(),
            limit: MAX_CONTROL_PAYLOAD,
        });
    }
    encode_raw(CONTROL_KIND, &payload)
}

pub fn encode_chunk(chunk: &Chunk) -> Result<Vec<u8>, ProtocolError> {
    if chunk.data.len() > MAX_CHUNK_DATA {
        return Err(ProtocolError::OversizedChunk {
            actual: chunk.data.len(),
            limit: MAX_CHUNK_DATA,
        });
    }
    let mut payload = Vec::with_capacity(CHUNK_FIXED_LEN + chunk.data.len());
    payload.extend_from_slice(chunk.batch_id.as_bytes());
    payload.extend_from_slice(chunk.entry_id.as_bytes());
    payload.extend_from_slice(&chunk.offset.to_be_bytes());
    payload.extend_from_slice(&(chunk.data.len() as u32).to_be_bytes());
    payload.extend_from_slice(&chunk.hash());
    payload.extend_from_slice(&chunk.data);
    encode_raw(CHUNK_KIND, &payload)
}

fn encode_raw(kind: u8, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let length = u32::try_from(payload.len()).map_err(|_| ProtocolError::OversizedFrame {
        actual: payload.len(),
        limit: u32::MAX as usize,
    })?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub fn decode_frame(input: &[u8]) -> Result<Frame, ProtocolError> {
    let expected = frame_len(input)?;
    if input.len() != expected {
        return Err(ProtocolError::TruncatedFrame);
    }
    let payload = &input[FRAME_HEADER_LEN..];
    match input[0] {
        CONTROL_KIND => serde_json::from_slice(payload)
            .map(Frame::Control)
            .map_err(|error| ProtocolError::InvalidControl(error.to_string())),
        CHUNK_KIND => decode_chunk(payload).map(Frame::Chunk),
        kind => Err(ProtocolError::UnknownFrameKind(kind)),
    }
}

pub fn decode_chunk(payload: &[u8]) -> Result<Chunk, ProtocolError> {
    if payload.len() < CHUNK_FIXED_LEN {
        return Err(ProtocolError::InvalidChunk);
    }
    let declared_len =
        u32::from_be_bytes(payload[40..44].try_into().expect("fixed chunk prefix")) as usize;
    if declared_len > MAX_CHUNK_DATA {
        return Err(ProtocolError::OversizedChunk {
            actual: declared_len,
            limit: MAX_CHUNK_DATA,
        });
    }
    if payload.len() != CHUNK_FIXED_LEN + declared_len {
        return Err(ProtocolError::InvalidChunk);
    }
    let batch_id = Uuid::from_slice(&payload[0..16]).map_err(|_| ProtocolError::InvalidChunk)?;
    let entry_id = Uuid::from_slice(&payload[16..32]).map_err(|_| ProtocolError::InvalidChunk)?;
    let offset = u64::from_be_bytes(payload[32..40].try_into().expect("fixed chunk prefix"));
    let expected_hash: [u8; 32] = payload[44..76].try_into().expect("fixed chunk prefix");
    let data = payload[CHUNK_FIXED_LEN..].to_vec();
    if *blake3::hash(&data).as_bytes() != expected_hash {
        return Err(ProtocolError::ChunkHashMismatch);
    }
    Ok(Chunk {
        batch_id,
        entry_id,
        offset,
        data,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    Hello(Hello),
    Auth(Auth),
    PairRequest(PairRequest),
    PairProof(PairProof),
    PairConfirmed(PairConfirmed),
    PairRejected(PairRejected),
    OfferStart(OfferStart),
    ManifestPage(ManifestPage),
    OfferAccept(OfferAccept),
    OfferReject(OfferReject),
    ChunkAck(ChunkAck),
    EntryComplete(EntryComplete),
    EntryVerified(EntryVerified),
    BatchComplete(BatchComplete),
    BatchReceipt(BatchReceipt),
    Pause(Pause),
    ResumeQuery(ResumeQuery),
    Cancel(Cancel),
    Ping(Ping),
    Pong(Pong),
    Error(WireError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol_min: u16,
    pub protocol_max: u16,
    pub device_id: String,
    pub display_name: String,
    pub session_nonce: String,
    pub capabilities: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Auth {
    pub transcript: String,
    pub signature: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRequest {
    pub session_id: Uuid,
    pub device_name: String,
    pub nonce: String,
    pub expires_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairProof {
    pub session_id: Uuid,
    pub transcript_hash: String,
    pub signature: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairConfirmed {
    pub session_id: Uuid,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRejected {
    pub session_id: Uuid,
    pub reason_code: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopLevelItem {
    pub entry_id: Uuid,
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferStart {
    pub batch_id: Uuid,
    pub items: Vec<TopLevelItem>,
    pub total_bytes: u64,
    pub total_entries: u64,
    pub created_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub entry_id: Uuid,
    pub parent_entry_id: Option<Uuid>,
    pub kind: EntryKind,
    pub components: Vec<String>,
    pub size: u64,
    pub mtime: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestPage {
    pub batch_id: Uuid,
    pub page: u32,
    pub entries: Vec<ManifestEntry>,
    pub final_page: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferAccept {
    pub batch_id: Uuid,
    pub destination_generation: u64,
    pub resolved_top_level_names: Vec<String>,
    pub available_space_ok: bool,
    pub checkpoints: Vec<Checkpoint>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub entry_id: Uuid,
    pub durable_offset: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferReject {
    pub batch_id: Uuid,
    pub code: String,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkAck {
    pub batch_id: Uuid,
    pub entry_id: Uuid,
    pub durable_offset: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryComplete {
    pub batch_id: Uuid,
    pub entry_id: Uuid,
    pub total_size: u64,
    pub blake3: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryVerified {
    pub batch_id: Uuid,
    pub entry_id: Uuid,
    pub relative_destination: Vec<String>,
    pub blake3: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchComplete {
    pub batch_id: Uuid,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchReceipt {
    pub batch_id: Uuid,
    pub result: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pause {
    pub batch_id: Uuid,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeQuery {
    pub batch_id: Uuid,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cancel {
    pub batch_id: Uuid,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ping {
    pub nonce: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pong {
    pub nonce: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireError {
    pub code: String,
    pub retryable: bool,
    pub detail: String,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SessionPhase {
    New,
    HelloExchanged,
    Authenticated,
    Pairing,
    Offering,
    OfferAccepted,
    Closed,
}

impl fmt::Display for SessionPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone)]
pub struct SessionValidator {
    phase: SessionPhase,
}

impl Default for SessionValidator {
    fn default() -> Self {
        Self::new()
    }
}
impl SessionValidator {
    pub fn new() -> Self {
        Self {
            phase: SessionPhase::New,
        }
    }
    pub fn phase(&self) -> SessionPhase {
        self.phase
    }
    pub fn validate(&mut self, frame: &Frame) -> Result<(), ProtocolError> {
        let message = match frame {
            Frame::Control(message) => message,
            Frame::Chunk(_) => return self.transition_chunk(),
        };
        let name = message.name();
        use ControlMessage::*;
        match message {
            Hello(hello) => {
                if hello.protocol_min > PROTOCOL_VERSION || hello.protocol_max < PROTOCOL_VERSION {
                    return Err(ProtocolError::UnsupportedVersion(PROTOCOL_VERSION));
                }
                self.transition(
                    &[SessionPhase::New, SessionPhase::HelloExchanged],
                    SessionPhase::HelloExchanged,
                    name,
                )
            }
            Auth(_) => self.transition(
                &[SessionPhase::HelloExchanged],
                SessionPhase::Authenticated,
                name,
            ),
            PairRequest(_) | PairProof(_) => self.transition(
                &[SessionPhase::Authenticated, SessionPhase::Pairing],
                SessionPhase::Pairing,
                name,
            ),
            PairConfirmed(_) => {
                self.transition(&[SessionPhase::Pairing], SessionPhase::Authenticated, name)
            }
            PairRejected(_) => {
                self.transition(&[SessionPhase::Pairing], SessionPhase::Closed, name)
            }
            OfferStart(_) => {
                self.transition(&[SessionPhase::Authenticated], SessionPhase::Offering, name)
            }
            ManifestPage(_) => {
                self.transition(&[SessionPhase::Offering], SessionPhase::Offering, name)
            }
            OfferAccept(_) => {
                self.transition(&[SessionPhase::Offering], SessionPhase::OfferAccepted, name)
            }
            OfferReject(_) => {
                self.transition(&[SessionPhase::Offering], SessionPhase::Authenticated, name)
            }
            ChunkAck(_) | EntryComplete(_) | EntryVerified(_) | BatchComplete(_)
            | BatchReceipt(_) | Pause(_) | ResumeQuery(_) | Cancel(_) => {
                self.require(&[SessionPhase::OfferAccepted], name)
            }
            Ping(_) | Pong(_) | Error(_) => self.require(
                &[
                    SessionPhase::HelloExchanged,
                    SessionPhase::Authenticated,
                    SessionPhase::Pairing,
                    SessionPhase::Offering,
                    SessionPhase::OfferAccepted,
                ],
                name,
            ),
        }
    }
    fn transition_chunk(&mut self) -> Result<(), ProtocolError> {
        self.require(&[SessionPhase::OfferAccepted], "chunk")
    }
    fn require(
        &self,
        allowed: &[SessionPhase],
        message: &'static str,
    ) -> Result<(), ProtocolError> {
        if allowed.contains(&self.phase) {
            Ok(())
        } else {
            Err(ProtocolError::InvalidState {
                message,
                phase: self.phase,
            })
        }
    }
    fn transition(
        &mut self,
        allowed: &[SessionPhase],
        next: SessionPhase,
        message: &'static str,
    ) -> Result<(), ProtocolError> {
        self.require(allowed, message)?;
        self.phase = next;
        Ok(())
    }
}

impl ControlMessage {
    fn name(&self) -> &'static str {
        match self {
            Self::Hello(_) => "hello",
            Self::Auth(_) => "auth",
            Self::PairRequest(_) => "pair_request",
            Self::PairProof(_) => "pair_proof",
            Self::PairConfirmed(_) => "pair_confirmed",
            Self::PairRejected(_) => "pair_rejected",
            Self::OfferStart(_) => "offer_start",
            Self::ManifestPage(_) => "manifest_page",
            Self::OfferAccept(_) => "offer_accept",
            Self::OfferReject(_) => "offer_reject",
            Self::ChunkAck(_) => "chunk_ack",
            Self::EntryComplete(_) => "entry_complete",
            Self::EntryVerified(_) => "entry_verified",
            Self::BatchComplete(_) => "batch_complete",
            Self::BatchReceipt(_) => "batch_receipt",
            Self::Pause(_) => "pause",
            Self::ResumeQuery(_) => "resume_query",
            Self::Cancel(_) => "cancel",
            Self::Ping(_) => "ping",
            Self::Pong(_) => "pong",
            Self::Error(_) => "error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> Uuid {
        Uuid::from_u128(42)
    }
    fn hello() -> Frame {
        Frame::Control(ControlMessage::Hello(Hello {
            protocol_min: 1,
            protocol_max: 1,
            device_id: "device".into(),
            display_name: "Desk".into(),
            session_nonce: "nonce".into(),
            capabilities: vec!["transfer".into()],
        }))
    }
    fn offer() -> Frame {
        Frame::Control(ControlMessage::OfferStart(OfferStart {
            batch_id: id(),
            items: vec![],
            total_bytes: 0,
            total_entries: 0,
            created_at: "2026-01-01T00:00:00Z".into(),
        }))
    }
    #[test]
    fn preface_is_version_one() {
        assert_eq!(&PROTOCOL_PREFACE[..11], b"FILEPORTER\0");
        assert_eq!(
            u16::from_be_bytes(PROTOCOL_PREFACE[11..].try_into().unwrap()),
            1
        );
    }
    #[test]
    fn control_round_trip() {
        let message = match hello() {
            Frame::Control(v) => v,
            _ => unreachable!(),
        };
        assert_eq!(
            decode_frame(&encode_control(&message).unwrap()).unwrap(),
            Frame::Control(message)
        );
    }
    #[test]
    fn chunk_round_trip_and_hash_validation() {
        let frame = encode_chunk(&Chunk {
            batch_id: id(),
            entry_id: Uuid::from_u128(43),
            offset: 9,
            data: b"hello".to_vec(),
        })
        .unwrap();
        assert_eq!(
            decode_frame(&frame).unwrap(),
            Frame::Chunk(Chunk {
                batch_id: id(),
                entry_id: Uuid::from_u128(43),
                offset: 9,
                data: b"hello".to_vec()
            })
        );
        let mut bad = frame;
        *bad.last_mut().unwrap() ^= 1;
        assert_eq!(decode_frame(&bad), Err(ProtocolError::ChunkHashMismatch));
    }
    #[test]
    fn rejects_truncated_unknown_and_oversized_frames() {
        assert_eq!(
            decode_frame(&[CONTROL_KIND]),
            Err(ProtocolError::TruncatedFrame)
        );
        assert_eq!(
            frame_len(&[99, 0, 0, 0, 0]),
            Err(ProtocolError::UnknownFrameKind(99))
        );
        let mut oversize = vec![CONTROL_KIND];
        oversize.extend_from_slice(&((MAX_CONTROL_PAYLOAD + 1) as u32).to_be_bytes());
        assert!(matches!(
            frame_len(&oversize),
            Err(ProtocolError::OversizedFrame { .. })
        ));
    }
    #[test]
    fn rejects_bad_chunk_declared_length() {
        let mut frame = encode_chunk(&Chunk {
            batch_id: id(),
            entry_id: id(),
            offset: 0,
            data: vec![1],
        })
        .unwrap();
        frame[FRAME_HEADER_LEN + 40..FRAME_HEADER_LEN + 44].copy_from_slice(&2u32.to_be_bytes());
        assert_eq!(decode_frame(&frame), Err(ProtocolError::InvalidChunk));
    }
    #[test]
    fn rejects_incompatible_protocol_version() {
        let frame = Frame::Control(ControlMessage::Hello(Hello {
            protocol_min: 2,
            protocol_max: 2,
            device_id: "a".into(),
            display_name: "a".into(),
            session_nonce: "n".into(),
            capabilities: vec![],
        }));
        assert_eq!(
            SessionValidator::new().validate(&frame),
            Err(ProtocolError::UnsupportedVersion(1))
        );
    }
    #[test]
    fn state_machine_rejects_data_before_authentication_and_acceptance() {
        let chunk = Frame::Chunk(Chunk {
            batch_id: id(),
            entry_id: id(),
            offset: 0,
            data: vec![],
        });
        let mut validator = SessionValidator::new();
        assert!(matches!(
            validator.validate(&chunk),
            Err(ProtocolError::InvalidState { .. })
        ));
        validator.validate(&hello()).unwrap();
        validator
            .validate(&Frame::Control(ControlMessage::Auth(Auth {
                transcript: "t".into(),
                signature: "s".into(),
            })))
            .unwrap();
        assert!(matches!(
            validator.validate(&chunk),
            Err(ProtocolError::InvalidState { .. })
        ));
        validator.validate(&offer()).unwrap();
        validator
            .validate(&Frame::Control(ControlMessage::OfferAccept(OfferAccept {
                batch_id: id(),
                destination_generation: 1,
                resolved_top_level_names: vec![],
                available_space_ok: true,
                checkpoints: vec![],
            })))
            .unwrap();
        assert!(validator.validate(&chunk).is_ok());
    }
    #[test]
    fn invalid_control_transition_is_rejected() {
        let mut validator = SessionValidator::new();
        assert!(matches!(
            validator.validate(&offer()),
            Err(ProtocolError::InvalidState {
                message: "offer_start",
                phase: SessionPhase::New
            })
        ));
    }
}
