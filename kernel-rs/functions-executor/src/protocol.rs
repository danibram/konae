use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Write};

pub const ABI: &str = "konae-functions-ipc/2";
pub const STRICT_WORKER_BOOTSTRAP: [u8; 4] = *b"KFS1";
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_OPERATION_BYTES: usize = 1024 * 1024;
pub const BODY_CHUNK_BYTES: usize = 64 * 1024;

pub fn operation_fits(operation: &str, args: &Value) -> bool {
    operation.len() <= 128
        && serde_json::to_vec(args).is_ok_and(|bytes| bytes.len() <= MAX_OPERATION_BYTES)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerLimits {
    pub heap_bytes: usize,
    pub response_bytes: usize,
    pub rss_bytes: usize,
    pub operation_limit: usize,
}
impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            heap_bytes: 64 * 1024 * 1024,
            response_bytes: 2 * 1024 * 1024,
            rss_bytes: 256 * 1024 * 1024,
            operation_limit: 64,
        }
    }
}
impl WorkerLimits {
    pub fn validate(&self) -> bool {
        self.heap_bytes >= 1024 * 1024
            && self.heap_bytes <= self.rss_bytes
            && self.response_bytes > 0
            && self.response_bytes <= 2 * 1024 * 1024
            && self.operation_limit > 0
            && self.operation_limit <= 4096
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    #[serde(skip)]
    pub body: Vec<u8>,
    #[serde(default)]
    pub body_len: usize,
    pub actor: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyChunk {
    pub bytes: Vec<u8>,
    pub done: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub generation: String,
    pub request: u64,
    pub token: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ParentMessage {
    Init {
        abi: String,
        function: String,
        source: String,
        generation: String,
        limits: WorkerLimits,
    },
    Invoke {
        capability: Capability,
        input: Input,
    },
    Spin {
        capability: Capability,
    },
    OperationResult {
        capability: Capability,
        sequence: u64,
        result: Result<Value, String>,
    },
    Cancel {
        capability: Capability,
    },
    BodyChunk {
        capability: Capability,
        sequence: u64,
        body_len: usize,
        #[serde(default)]
        bytes: Vec<u8>,
        done: bool,
    },
    Shutdown {
        grace_ms: u64,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ChildMessage {
    Ready {
        abi: String,
        generation: String,
    },
    Operation {
        capability: Capability,
        sequence: u64,
        operation: String,
        args: Value,
    },
    BodyRead {
        capability: Capability,
        sequence: u64,
        maximum: usize,
    },
    Reply {
        capability: Capability,
        output: Output,
    },
    Failure {
        capability: Option<Capability>,
        code: String,
    },
    ShutdownComplete {
        invoked: bool,
        error: Option<String>,
    },
}

pub fn encode<T: Serialize>(message: &T) -> io::Result<Vec<u8>> {
    let data = serde_json::to_vec(message).map_err(io::Error::other)?;
    if data.is_empty() || data.len() > MAX_FRAME_BYTES {
        return Err(io::Error::other("frame_too_large"));
    }
    let mut frame = Vec::with_capacity(data.len() + 4);
    frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
    frame.extend_from_slice(&data);
    Ok(frame)
}
pub fn encode_parent(message: &ParentMessage) -> io::Result<Vec<u8>> {
    if let ParentMessage::BodyChunk {
        capability,
        sequence,
        body_len,
        bytes,
        done,
    } = message
    {
        #[derive(Serialize)]
        struct Header<'a> {
            kind: &'static str,
            capability: &'a Capability,
            sequence: u64,
            body_len: usize,
            done: bool,
        }
        let header = serde_json::to_vec(&Header {
            kind: "BodyChunk",
            capability,
            sequence: *sequence,
            body_len: *body_len,
            done: *done,
        })
        .map_err(io::Error::other)?;
        let payload_len = 1usize
            .saturating_add(4)
            .saturating_add(header.len())
            .saturating_add(bytes.len());
        if payload_len > MAX_FRAME_BYTES
            || bytes.len() > BODY_CHUNK_BYTES
            || *body_len != bytes.len()
        {
            return Err(io::Error::other("frame_too_large"));
        }
        let mut frame = Vec::with_capacity(payload_len + 4);
        frame.extend_from_slice(&(payload_len as u32).to_be_bytes());
        frame.push(0xff);
        frame.extend_from_slice(&(header.len() as u32).to_be_bytes());
        frame.extend_from_slice(&header);
        frame.extend_from_slice(bytes);
        return Ok(frame);
    }
    encode(message)
}
pub fn read<R: Read, T: for<'de> Deserialize<'de>>(reader: &mut R) -> io::Result<T> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let len = u32::from_be_bytes(prefix) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::other("invalid_frame_length"));
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    if bytes.first() == Some(&0xff) {
        return Err(io::Error::other("binary_parent_frame"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}
pub fn read_parent<R: Read>(reader: &mut R) -> io::Result<ParentMessage> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let len = u32::from_be_bytes(prefix) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::other("invalid_frame_length"));
    }
    let mut frame = vec![0; len];
    reader.read_exact(&mut frame)?;
    if frame.first() != Some(&0xff) {
        return serde_json::from_slice(&frame).map_err(io::Error::other);
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Header {
        kind: String,
        capability: Capability,
        sequence: u64,
        body_len: usize,
        done: bool,
    }
    if frame.len() < 5 {
        return Err(io::Error::other("invalid_binary_frame"));
    }
    let header_len = u32::from_be_bytes(frame[1..5].try_into().unwrap()) as usize;
    let header_end = 5usize
        .checked_add(header_len)
        .ok_or_else(|| io::Error::other("invalid_binary_frame"))?;
    if header_len == 0 || header_end > frame.len() {
        return Err(io::Error::other("invalid_binary_frame"));
    }
    let header: Header = serde_json::from_slice(&frame[5..header_end]).map_err(io::Error::other)?;
    let body = frame.split_off(header_end);
    if header.kind != "BodyChunk"
        || header.body_len != body.len()
        || body.len() > BODY_CHUNK_BYTES
        || (header.done && !body.is_empty())
    {
        return Err(io::Error::other("invalid_binary_frame"));
    }
    Ok(ParentMessage::BodyChunk {
        capability: header.capability,
        sequence: header.sequence,
        body_len: header.body_len,
        bytes: body,
        done: header.done,
    })
}
pub fn write<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    writer.write_all(&encode(message)?)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rejects_oversized_frame_before_reading_payload() {
        let bytes = (MAX_FRAME_BYTES as u32 + 1).to_be_bytes();
        let mut reader = Cursor::new(bytes);
        assert!(read::<_, ParentMessage>(&mut reader).is_err());
        assert_eq!(reader.position(), 4);
    }

    #[test]
    fn rejects_truncated_frame_and_unknown_fields() {
        let mut short = Cursor::new([0, 0, 0, 8, b'{']);
        assert!(read::<_, ParentMessage>(&mut short).is_err());
        let data = br#"{"kind":"Shutdown","grace_ms":10,"actor":"owner"}"#;
        let mut bytes = (data.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(data);
        assert!(read::<_, ParentMessage>(&mut Cursor::new(bytes)).is_err());
    }

    #[test]
    fn preserves_operation_error_and_capability_on_roundtrip() {
        let capability = Capability {
            generation: "worker-1".into(),
            request: 2,
            token: "request-2".into(),
        };
        let message = ParentMessage::OperationResult {
            capability: capability.clone(),
            sequence: 3,
            result: Err("idempotency_conflict".into()),
        };
        let decoded: ParentMessage = read(&mut Cursor::new(encode(&message).unwrap())).unwrap();
        match decoded {
            ParentMessage::OperationResult {
                capability: actual,
                sequence,
                result,
            } => {
                assert_eq!(actual, capability);
                assert_eq!(sequence, 3);
                assert_eq!(result.unwrap_err(), "idempotency_conflict");
            }
            _ => panic!("unexpected frame"),
        }
    }

    #[test]
    fn body_chunk_uses_bounded_binary_payload_and_rejects_malformed_length() {
        let message = ParentMessage::BodyChunk {
            capability: Capability {
                generation: "worker-1".into(),
                request: 1,
                token: "token".into(),
            },
            sequence: 1,
            body_len: BODY_CHUNK_BYTES,
            bytes: vec![0xff; BODY_CHUNK_BYTES],
            done: false,
        };
        let frame = encode_parent(&message).unwrap();
        assert_eq!(frame[4], 0xff);
        let decoded = read_parent(&mut Cursor::new(frame)).unwrap();
        match decoded {
            ParentMessage::BodyChunk {
                body_len, bytes, ..
            } => {
                assert_eq!(body_len, BODY_CHUNK_BYTES);
                assert_eq!(bytes, vec![0xff; BODY_CHUNK_BYTES]);
            }
            _ => panic!("unexpected message"),
        }
        let malformed = encode_parent(&ParentMessage::BodyChunk {
            capability: Capability {
                generation: "worker-1".into(),
                request: 1,
                token: "token".into(),
            },
            sequence: 1,
            body_len: 1,
            bytes: vec![0],
            done: false,
        })
        .unwrap();
        let mut malformed = malformed;
        let header_len = u32::from_be_bytes(malformed[5..9].try_into().unwrap()) as usize;
        let header_end = 9 + header_len;
        let header = std::str::from_utf8(&malformed[9..header_end]).unwrap();
        let broken = header.replace("\"body_len\":1", "\"body_len\":2");
        assert_ne!(broken, header);
        malformed[9..header_end].copy_from_slice(broken.as_bytes());
        assert!(read_parent(&mut Cursor::new(malformed)).is_err());
    }

    #[test]
    fn operation_budget_counts_json_escaping_and_all_fields() {
        let single =
            serde_json::json!({"name":"orders.update","input":{"note":"\0".repeat(100_000)}});
        assert!(operation_fits("action", &single));
        let multiple = serde_json::json!({"name":"orders.update","input":{"first":"\0".repeat(100_000),"second":"\0".repeat(100_000)}});
        assert!(!operation_fits("action", &multiple));
        let too_large = serde_json::json!({"value":"x".repeat(MAX_OPERATION_BYTES)});
        assert!(!operation_fits("action", &too_large));
    }
}
