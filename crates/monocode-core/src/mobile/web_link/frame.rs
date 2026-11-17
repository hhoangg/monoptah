//! The frame codec of the web link: the outer relay frame (architecture.md
//! 5.1), the Noise plaintext layout with chunking and reassembly (5.2), and the
//! relay close and error codes. It mirrors `web/protocol/src/{frame,chunk,
//! limits,close,errors}.ts`; both sides are tested against
//! `web/protocol/vectors.json`.
use std::collections::HashMap;

// --- Limits (limits.ts) ---

pub const FRAME_VERSION: u8 = 1;
/// version(1) + type(1) + channel(2) + deviceId(16).
pub const FRAME_HEADER_SIZE: usize = 20;
pub const MIN_FRAME_SIZE: usize = FRAME_HEADER_SIZE;
pub const MAX_FRAME_BODY: usize = 65_535;
pub const MAX_FRAME_SIZE: usize = FRAME_HEADER_SIZE + MAX_FRAME_BODY;
/// The largest Noise message minus the 16-byte tag, kind byte included.
pub const MAX_NOISE_PLAINTEXT: usize = 65_519;
/// kind(1) + msgId(4) + index(2) + flags(1).
pub const CHUNK_HEADER_SIZE: usize = 8;
pub const MAX_MSG_PAYLOAD: usize = MAX_NOISE_PLAINTEXT - 1;
pub const MAX_CHUNK_DATA: usize = MAX_NOISE_PLAINTEXT - CHUNK_HEADER_SIZE;
pub const CHUNK_FLAG_LAST: u8 = 0x01;
/// Reassembled message limit, both directions.
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_REASSEMBLY_IN_FLIGHT: usize = 4;
pub const REASSEMBLY_TIMEOUT_MS: u64 = 60_000;

// --- Outer frame ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Hs1 = 0x01,
    Hs2 = 0x02,
    Transport = 0x03,
    Bye = 0x04,
    Ctrl = 0x10,
    Error = 0x11,
}

impl FrameType {
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0x01 => Self::Hs1,
            0x02 => Self::Hs2,
            0x03 => Self::Transport,
            0x04 => Self::Bye,
            0x10 => Self::Ctrl,
            0x11 => Self::Error,
            _ => return None,
        })
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    /// The name used in the TypeScript contract.
    pub fn name(self) -> &'static str {
        match self {
            Self::Hs1 => "HS1",
            Self::Hs2 => "HS2",
            Self::Transport => "TRANSPORT",
            Self::Bye => "BYE",
            Self::Ctrl => "CTRL",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    UnknownVersion,
    UnknownType,
    TooShort,
    TooLong,
}

impl FrameError {
    /// The error kind named in vectors.json.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnknownVersion => "unknown_version",
            Self::UnknownType => "unknown_type",
            Self::TooShort => "too_short",
            Self::TooLong => "too_long",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameType,
    pub channel: u16,
    pub device_id: [u8; 16],
    pub body: Vec<u8>,
}

impl Frame {
    /// Fails with `TooLong` when the body exceeds 65,535 bytes.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        if self.body.len() > MAX_FRAME_BODY {
            return Err(FrameError::TooLong);
        }
        let mut out = Vec::with_capacity(FRAME_HEADER_SIZE + self.body.len());
        out.push(FRAME_VERSION);
        out.push(self.kind.code());
        out.extend_from_slice(&self.channel.to_be_bytes());
        out.extend_from_slice(&self.device_id);
        out.extend_from_slice(&self.body);
        Ok(out)
    }

    /// Check order: length, version, type.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        if bytes.len() < MIN_FRAME_SIZE {
            return Err(FrameError::TooShort);
        }
        if bytes.len() > MAX_FRAME_SIZE {
            return Err(FrameError::TooLong);
        }
        if bytes[0] != FRAME_VERSION {
            return Err(FrameError::UnknownVersion);
        }
        let kind = FrameType::from_code(bytes[1]).ok_or(FrameError::UnknownType)?;
        let mut device_id = [0u8; 16];
        device_id.copy_from_slice(&bytes[4..FRAME_HEADER_SIZE]);
        Ok(Self {
            kind,
            channel: u16::from_be_bytes([bytes[2], bytes[3]]),
            device_id,
            body: bytes[FRAME_HEADER_SIZE..].to_vec(),
        })
    }
}

// --- Plaintext ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PlaintextKind {
    Msg = 0x01,
    Chunk = 0x02,
    Rekey = 0x03,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaintextError {
    Empty,
    UnknownKind,
    TooShort,
    TooLong,
    RekeyNotEmpty,
}

impl PlaintextError {
    /// The error kind named in vectors.json.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::UnknownKind => "unknown_kind",
            Self::TooShort => "too_short",
            Self::TooLong => "too_long",
            Self::RekeyNotEmpty => "rekey_not_empty",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plaintext {
    Msg(Vec<u8>),
    /// Flag bits other than `last` are ignored on decode.
    Chunk {
        msg_id: u32,
        index: u16,
        last: bool,
        data: Vec<u8>,
    },
    Rekey,
}

impl Plaintext {
    pub fn encode(&self) -> Result<Vec<u8>, PlaintextError> {
        match self {
            Self::Msg(payload) => {
                if payload.len() > MAX_MSG_PAYLOAD {
                    return Err(PlaintextError::TooLong);
                }
                let mut out = Vec::with_capacity(1 + payload.len());
                out.push(PlaintextKind::Msg as u8);
                out.extend_from_slice(payload);
                Ok(out)
            }
            Self::Chunk {
                msg_id,
                index,
                last,
                data,
            } => {
                if data.len() > MAX_CHUNK_DATA {
                    return Err(PlaintextError::TooLong);
                }
                let mut out = Vec::with_capacity(CHUNK_HEADER_SIZE + data.len());
                out.push(PlaintextKind::Chunk as u8);
                out.extend_from_slice(&msg_id.to_be_bytes());
                out.extend_from_slice(&index.to_be_bytes());
                out.push(if *last { CHUNK_FLAG_LAST } else { 0 });
                out.extend_from_slice(data);
                Ok(out)
            }
            Self::Rekey => Ok(vec![PlaintextKind::Rekey as u8]),
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PlaintextError> {
        let Some(&kind) = bytes.first() else {
            return Err(PlaintextError::Empty);
        };
        if bytes.len() > MAX_NOISE_PLAINTEXT {
            return Err(PlaintextError::TooLong);
        }
        match kind {
            k if k == PlaintextKind::Msg as u8 => Ok(Self::Msg(bytes[1..].to_vec())),
            k if k == PlaintextKind::Chunk as u8 => {
                if bytes.len() < CHUNK_HEADER_SIZE {
                    return Err(PlaintextError::TooShort);
                }
                Ok(Self::Chunk {
                    msg_id: u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]),
                    index: u16::from_be_bytes([bytes[5], bytes[6]]),
                    last: bytes[7] & CHUNK_FLAG_LAST != 0,
                    data: bytes[CHUNK_HEADER_SIZE..].to_vec(),
                })
            }
            k if k == PlaintextKind::Rekey as u8 => {
                if bytes.len() != 1 {
                    return Err(PlaintextError::RekeyNotEmpty);
                }
                Ok(Self::Rekey)
            }
            _ => Err(PlaintextError::UnknownKind),
        }
    }
}

/// A message above the limit cannot be split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLarge;

/// Splits one serialized message into encoded plaintexts of at most 65,519
/// bytes: one MSG when it fits, else CHUNKs from index 0 with `last` on the
/// final one. Fails above `max_bytes` (use [`MAX_REQUEST_BYTES`]).
pub fn split_message(
    bytes: &[u8],
    msg_id: u32,
    max_bytes: usize,
) -> Result<Vec<Vec<u8>>, TooLarge> {
    if bytes.len() > max_bytes {
        return Err(TooLarge);
    }
    if bytes.len() <= MAX_MSG_PAYLOAD {
        let part = Plaintext::Msg(bytes.to_vec())
            .encode()
            .map_err(|_| TooLarge)?;
        return Ok(vec![part]);
    }
    let mut parts = Vec::new();
    for (index, data) in bytes.chunks(MAX_CHUNK_DATA).enumerate() {
        let last = (index + 1) * MAX_CHUNK_DATA >= bytes.len();
        let index = u16::try_from(index).map_err(|_| TooLarge)?;
        parts.push(
            Plaintext::Chunk {
                msg_id,
                index,
                last,
                data: data.to_vec(),
            }
            .encode()
            .map_err(|_| TooLarge)?,
        );
    }
    Ok(parts)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReassemblyError {
    TooLarge,
    TooManyInFlight,
    Timeout,
    OutOfOrder,
    /// The plaintext itself was malformed.
    Plaintext(PlaintextError),
}

/// What a pushed plaintext produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pushed {
    Message(Vec<u8>),
    Partial,
    Rekey,
}

struct Pending {
    started_at: u64,
    next_index: u32,
    size: usize,
    parts: Vec<Vec<u8>>,
}

/// Collects CHUNKs by message id: indexes arrive in order from 0, at most 4
/// messages in flight, 8 MiB per message, 60 s from the first chunk. The clock
/// is passed in (milliseconds), so tests need no real time. Any error ends the
/// session. One instance per session and direction.
pub struct Reassembler {
    max_bytes: usize,
    max_in_flight: usize,
    timeout_ms: u64,
    pending: HashMap<u32, Pending>,
}

impl Default for Reassembler {
    fn default() -> Self {
        Self::new(
            MAX_REQUEST_BYTES,
            MAX_REASSEMBLY_IN_FLIGHT,
            REASSEMBLY_TIMEOUT_MS,
        )
    }
}

impl Reassembler {
    pub fn new(max_bytes: usize, max_in_flight: usize, timeout_ms: u64) -> Self {
        Self {
            max_bytes,
            max_in_flight,
            timeout_ms,
            pending: HashMap::new(),
        }
    }

    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// Fails with `Timeout` when a message has been in flight too long.
    pub fn check_timeouts(&self, now_ms: u64) -> Result<(), ReassemblyError> {
        if self
            .pending
            .values()
            .any(|p| now_ms.saturating_sub(p.started_at) > self.timeout_ms)
        {
            return Err(ReassemblyError::Timeout);
        }
        Ok(())
    }

    /// Feeds one encoded plaintext at time `now_ms`.
    pub fn push(&mut self, plaintext: &[u8], now_ms: u64) -> Result<Pushed, ReassemblyError> {
        self.check_timeouts(now_ms)?;
        match Plaintext::decode(plaintext).map_err(ReassemblyError::Plaintext)? {
            Plaintext::Rekey => Ok(Pushed::Rekey),
            Plaintext::Msg(payload) => Ok(Pushed::Message(payload)),
            Plaintext::Chunk {
                msg_id,
                index,
                last,
                data,
            } => {
                if !self.pending.contains_key(&msg_id) {
                    if index != 0 {
                        return Err(ReassemblyError::OutOfOrder);
                    }
                    if self.pending.len() >= self.max_in_flight {
                        return Err(ReassemblyError::TooManyInFlight);
                    }
                    self.pending.insert(
                        msg_id,
                        Pending {
                            started_at: now_ms,
                            next_index: 0,
                            size: 0,
                            parts: Vec::new(),
                        },
                    );
                }
                let pending = self.pending.get_mut(&msg_id).expect("inserted above");
                if u32::from(index) != pending.next_index {
                    return Err(ReassemblyError::OutOfOrder);
                }
                pending.size += data.len();
                if pending.size > self.max_bytes {
                    return Err(ReassemblyError::TooLarge);
                }
                pending.parts.push(data);
                pending.next_index += 1;
                if !last {
                    return Ok(Pushed::Partial);
                }
                let pending = self.pending.remove(&msg_id).expect("present");
                Ok(Pushed::Message(pending.parts.concat()))
            }
        }
    }

    /// Drops everything in flight (session closed).
    pub fn reset(&mut self) {
        self.pending.clear();
    }
}

// --- Close and error codes (close.ts, errors.ts) ---

/// WebSocket close codes of the relay (architecture.md 4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseCode {
    Normal,
    GoingAway,
    Policy,
    MessageTooBig,
    Unauthorized,
    Revoked,
    RateLimited,
    Replaced,
    RoomQuota,
    Blocked,
    ProtocolError,
}

impl CloseCode {
    pub const ALL: [CloseCode; 11] = [
        Self::Normal,
        Self::GoingAway,
        Self::Policy,
        Self::MessageTooBig,
        Self::Unauthorized,
        Self::Revoked,
        Self::RateLimited,
        Self::Replaced,
        Self::RoomQuota,
        Self::Blocked,
        Self::ProtocolError,
    ];

    pub fn code(self) -> u16 {
        match self {
            Self::Normal => 1000,
            Self::GoingAway => 1001,
            Self::Policy => 1008,
            Self::MessageTooBig => 1009,
            Self::Unauthorized => 4001,
            Self::Revoked => 4003,
            Self::RateLimited => 4008,
            Self::Replaced => 4009,
            Self::RoomQuota => 4010,
            Self::Blocked => 4011,
            Self::ProtocolError => 4012,
        }
    }

    pub fn from_code(code: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.code() == code)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::GoingAway => "going_away",
            Self::Policy => "policy",
            Self::MessageTooBig => "message_too_big",
            Self::Unauthorized => "unauthorized",
            Self::Revoked => "revoked",
            Self::RateLimited => "rate_limited",
            Self::Replaced => "replaced",
            Self::RoomQuota => "room_quota",
            Self::Blocked => "blocked",
            Self::ProtocolError => "protocol_error",
        }
    }
}

/// The desktop `code` values (the same names as `mobile::policy::Code`).
pub const DESKTOP_ERROR_CODES: [&str; 13] = [
    "auto_stopping",
    "busy",
    "disabled",
    "forbidden",
    "instance_busy",
    "not_selected",
    "path_denied",
    "read_only",
    "revoked",
    "starting",
    "too_large",
    "unsupported",
    "unsupported_target",
];

/// Capability names a desktop may advertise in the `state` message
/// (`web/protocol` `KNOWN_CAPABILITIES`).
pub const KNOWN_CAPABILITIES: &[&str] = &[
    "approvals",
    "cloud",
    "cloudStart",
    "cloudMove",
    "gitPush",
    "monoTools",
    "monoFiles",
];

/// Codes in the ERROR relay frame body.
pub const RELAY_ERROR_CODES: [&str; 2] = ["desktop_offline", "unsupported_version"];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../../../web/protocol/vectors.json")).unwrap()
    }

    fn unhex(hex: &str) -> Vec<u8> {
        assert!(hex.len().is_multiple_of(2));
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// `{"hex"}` or `{"prefix","fill","length"}`.
    fn bytes(spec: &Value) -> Vec<u8> {
        if let Some(hex) = spec["hex"].as_str() {
            return unhex(hex);
        }
        let mut out = unhex(spec["prefix"].as_str().unwrap());
        let fill = unhex(spec["fill"].as_str().unwrap())[0];
        out.resize(spec["length"].as_u64().unwrap() as usize, fill);
        out
    }

    fn uuid(text: &str) -> [u8; 16] {
        unhex(&text.replace('-', "")).try_into().unwrap()
    }

    #[test]
    fn constants_match_the_contract() {
        let v = vectors();
        let c = &v["constants"];
        let n = |k: &str| c[k].as_u64().unwrap() as usize;
        assert_eq!(n("frameVersion"), FRAME_VERSION as usize);
        assert_eq!(n("frameHeaderSize"), FRAME_HEADER_SIZE);
        assert_eq!(n("minFrameSize"), MIN_FRAME_SIZE);
        assert_eq!(n("maxFrameBody"), MAX_FRAME_BODY);
        assert_eq!(n("maxFrameSize"), MAX_FRAME_SIZE);
        assert_eq!(n("maxNoisePlaintext"), MAX_NOISE_PLAINTEXT);
        assert_eq!(n("maxMsgPayload"), MAX_MSG_PAYLOAD);
        assert_eq!(n("maxChunkData"), MAX_CHUNK_DATA);
        assert_eq!(n("maxReassembledBytes"), MAX_REQUEST_BYTES);
        for (name, code) in c["frameTypes"].as_object().unwrap() {
            let kind = FrameType::from_code(code.as_u64().unwrap() as u8).unwrap();
            assert_eq!(kind.name(), name);
        }
        assert_eq!(c["frameTypes"].as_object().unwrap().len(), 6);
    }

    #[test]
    fn valid_frames_encode_and_decode_exactly() {
        let v = vectors();
        let list = v["frames"]["valid"].as_array().unwrap();
        assert!(!list.is_empty());
        for item in list {
            let name = item["name"].as_str().unwrap();
            let wire = bytes(&item["frame"]);
            let d = &item["decoded"];
            let frame = Frame {
                kind: FrameType::from_code(d["typeCode"].as_u64().unwrap() as u8).unwrap(),
                channel: d["channel"].as_u64().unwrap() as u16,
                device_id: uuid(d["deviceId"].as_str().unwrap()),
                body: bytes(&d["body"]),
            };
            assert_eq!(frame.kind.name(), d["type"].as_str().unwrap(), "{name}");
            assert_eq!(frame.encode().unwrap(), wire, "{name}");
            assert_eq!(Frame::decode(&wire).unwrap(), frame, "{name}");
        }
    }

    #[test]
    fn invalid_frames_fail_with_the_named_error() {
        let v = vectors();
        for item in v["frames"]["invalid"].as_array().unwrap() {
            let name = item["name"].as_str().unwrap();
            let err = Frame::decode(&bytes(&item["frame"])).unwrap_err();
            assert_eq!(err.as_str(), item["error"].as_str().unwrap(), "{name}");
        }
    }

    #[test]
    fn frame_size_bounds() {
        let frame = |len: usize| {
            let mut b = vec![FRAME_VERSION, FrameType::Transport.code(), 0, 1];
            b.resize(len, 0);
            b
        };
        assert!(Frame::decode(&frame(20)).is_ok());
        assert!(Frame::decode(&frame(65_555)).is_ok());
        assert_eq!(Frame::decode(&frame(19)), Err(FrameError::TooShort));
        assert_eq!(Frame::decode(&frame(65_556)), Err(FrameError::TooLong));
        let mut f = Frame::decode(&frame(65_555)).unwrap();
        assert_eq!(f.encode().unwrap().len(), 65_555);
        f.body.push(0);
        assert_eq!(f.encode(), Err(FrameError::TooLong));
    }

    #[test]
    fn plaintext_vectors_round_trip() {
        let v = vectors();
        for item in v["plaintext"]["valid"].as_array().unwrap() {
            let name = item["name"].as_str().unwrap();
            let wire = bytes(&item["plaintext"]);
            let d = &item["decoded"];
            let expected = match d["kind"].as_str().unwrap() {
                "msg" => Plaintext::Msg(bytes(&d["payload"])),
                "chunk" => Plaintext::Chunk {
                    msg_id: d["msgId"].as_u64().unwrap() as u32,
                    index: d["index"].as_u64().unwrap() as u16,
                    last: d["last"].as_bool().unwrap(),
                    data: bytes(&d["data"]),
                },
                "rekey" => Plaintext::Rekey,
                other => panic!("{other}"),
            };
            assert_eq!(Plaintext::decode(&wire).unwrap(), expected, "{name}");
            assert_eq!(expected.encode().unwrap(), wire, "{name}");
        }
        for item in v["plaintext"]["invalid"].as_array().unwrap() {
            let name = item["name"].as_str().unwrap();
            let err = Plaintext::decode(&bytes(&item["plaintext"])).unwrap_err();
            assert_eq!(err.as_str(), item["error"].as_str().unwrap(), "{name}");
        }
    }

    #[test]
    fn split_vectors_and_reassembly_round_trip() {
        let v = vectors();
        for item in v["plaintext"]["split"].as_array().unwrap() {
            let name = item["name"].as_str().unwrap();
            let fill = unhex(item["messageFill"].as_str().unwrap())[0];
            let message = vec![fill; item["messageLength"].as_u64().unwrap() as usize];
            let parts = split_message(
                &message,
                item["msgId"].as_u64().unwrap() as u32,
                MAX_REQUEST_BYTES,
            )
            .unwrap();
            let lengths: Vec<u64> = parts.iter().map(|p| p.len() as u64).collect();
            let want: Vec<u64> = item["partLengths"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_u64().unwrap())
                .collect();
            assert_eq!(lengths, want, "{name}");
            let mut reassembler = Reassembler::default();
            let mut done = None;
            for part in &parts {
                if let Pushed::Message(m) = reassembler.push(part, 0).unwrap() {
                    done = Some(m);
                }
            }
            assert_eq!(done.unwrap(), message, "{name}");
            assert_eq!(reassembler.in_flight(), 0);
        }
        assert_eq!(
            split_message(&vec![0; MAX_REQUEST_BYTES + 1], 1, MAX_REQUEST_BYTES),
            Err(TooLarge)
        );
    }

    fn chunk(msg_id: u32, index: u16, last: bool, len: usize) -> Vec<u8> {
        Plaintext::Chunk {
            msg_id,
            index,
            last,
            data: vec![1; len],
        }
        .encode()
        .unwrap()
    }

    #[test]
    fn reassembly_enforces_its_limits() {
        let mut r = Reassembler::default();
        assert_eq!(
            r.push(&chunk(1, 1, false, 1), 0),
            Err(ReassemblyError::OutOfOrder)
        );
        assert_eq!(r.push(&chunk(1, 0, false, 1), 0), Ok(Pushed::Partial));
        assert_eq!(
            r.push(&chunk(1, 2, false, 1), 0),
            Err(ReassemblyError::OutOfOrder)
        );
        // At most 4 in flight.
        let mut r = Reassembler::default();
        for id in 0..4 {
            assert_eq!(r.push(&chunk(id, 0, false, 1), 0), Ok(Pushed::Partial));
        }
        assert_eq!(
            r.push(&chunk(9, 0, false, 1), 0),
            Err(ReassemblyError::TooManyInFlight)
        );
        // 60 s from the first chunk, on an injected clock.
        assert_eq!(
            r.push(&chunk(0, 1, true, 1), 60_000),
            Ok(Pushed::Message(vec![1, 1]))
        );
        assert_eq!(
            r.push(&chunk(1, 1, true, 1), 60_001),
            Err(ReassemblyError::Timeout)
        );
        // 8 MiB per message.
        let mut r = Reassembler::default();
        let full = MAX_REQUEST_BYTES / MAX_CHUNK_DATA;
        for i in 0..full {
            assert_eq!(
                r.push(&chunk(5, i as u16, false, MAX_CHUNK_DATA), 0),
                Ok(Pushed::Partial)
            );
        }
        let rest = MAX_REQUEST_BYTES - full * MAX_CHUNK_DATA;
        assert!(r.push(&chunk(5, full as u16, true, rest + 1), 0).is_err());
        assert_eq!(Reassembler::default().push(&[3], 0), Ok(Pushed::Rekey));
        assert_eq!(
            Reassembler::default().push(&[], 0),
            Err(ReassemblyError::Plaintext(PlaintextError::Empty))
        );
    }

    #[test]
    fn capability_names_equal_the_contract() {
        let v = vectors();
        let theirs: Vec<&str> = v["state"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert_eq!(theirs, KNOWN_CAPABILITIES);
    }

    #[test]
    fn close_and_error_lists_equal_the_contract() {
        let v = vectors();
        let list = v["close"].as_array().unwrap();
        assert_eq!(list.len(), CloseCode::ALL.len(), "stale close code list");
        for item in list {
            let code = CloseCode::from_code(item["code"].as_u64().unwrap() as u16).unwrap();
            assert_eq!(code.as_str(), item["name"].as_str().unwrap());
        }
        let strings = |key: &str| -> Vec<String> {
            v["errors"][key]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(strings("desktop"), DESKTOP_ERROR_CODES);
        assert_eq!(strings("relay"), RELAY_ERROR_CODES);
    }
}
