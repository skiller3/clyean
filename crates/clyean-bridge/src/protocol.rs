// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The frames that travel over the bridge session.  Every frame is a big-endian `u32`
//! length (of everything after it), a `u8` kind, a `u32` stream id, and a payload.
//!
//! Streams opened by the bridge (connections accepted inside the container) have odd ids;
//! even ids are reserved for streams the host opens.  Each direction of a stream has a
//! window of [`STREAM_WINDOW`] bytes: a side sends data only while it holds credit, and
//! the receiver returns credit as it delivers the data, so a slow reader never makes the
//! other side buffer without bound.

use std::io::{self, Read};

/// Bumped whenever the frame format or its rules change; both ends must agree.
pub const PROTOCOL_VERSION: u16 = 1;

/// The largest data payload in one frame.
pub const MAX_DATA: usize = 64 * 1024;

/// The credit each direction of a stream starts with.
pub const STREAM_WINDOW: u32 = 256 * 1024;

/// The most streams the bridge keeps open at once; further connections are refused.
pub const MAX_STREAMS: usize = 64;

/// The largest frame either end accepts, which bounds the memory a single frame can take.
pub const MAX_FRAME_LEN: usize = 5 + MAX_DATA;

const KIND_HELLO: u8 = 1;
const KIND_OPEN: u8 = 2;
const KIND_DATA: u8 = 3;
const KIND_SHUTDOWN: u8 = 4;
const KIND_CLOSE: u8 = 5;
const KIND_CREDIT: u8 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// The bridge's first frame: the protocol it speaks and its release version.
    Hello { protocol: u16, version: String },
    /// A new stream on the named channel.
    Open { stream: u32, channel: String },
    /// Bytes for the stream's other end.
    Data { stream: u32, bytes: Vec<u8> },
    /// The sender will send no more data on the stream (a half-close).
    Shutdown { stream: u32 },
    /// The stream is gone, or was refused; the receiver drops its end at once.
    Close { stream: u32 },
    /// The sender delivered this many bytes and grants that much credit back.
    Credit { stream: u32, bytes: u32 },
}

impl Frame {
    pub fn hello() -> Self {
        Self::Hello {
            protocol: PROTOCOL_VERSION,
            version: crate::VERSION.to_string(),
        }
    }

    /// The complete encoding, length prefix included.
    pub fn encode(&self) -> Vec<u8> {
        let (kind, stream, payload): (u8, u32, Vec<u8>) = match self {
            Self::Hello { protocol, version } => {
                let mut payload = protocol.to_be_bytes().to_vec();
                payload.extend_from_slice(version.as_bytes());
                (KIND_HELLO, 0, payload)
            }
            Self::Open { stream, channel } => (KIND_OPEN, *stream, channel.as_bytes().to_vec()),
            Self::Data { stream, bytes } => (KIND_DATA, *stream, bytes.clone()),
            Self::Shutdown { stream } => (KIND_SHUTDOWN, *stream, Vec::new()),
            Self::Close { stream } => (KIND_CLOSE, *stream, Vec::new()),
            Self::Credit { stream, bytes } => (KIND_CREDIT, *stream, bytes.to_be_bytes().to_vec()),
        };
        let length = (5 + payload.len()) as u32;
        let mut encoded = Vec::with_capacity(4 + length as usize);
        encoded.extend_from_slice(&length.to_be_bytes());
        encoded.push(kind);
        encoded.extend_from_slice(&stream.to_be_bytes());
        encoded.extend_from_slice(&payload);
        encoded
    }

    /// Decodes the body of a frame (everything after the length prefix).
    pub fn decode(body: &[u8]) -> io::Result<Self> {
        if body.len() < 5 {
            return Err(invalid("a frame is shorter than its header"));
        }
        let kind = body[0];
        let stream = u32::from_be_bytes([body[1], body[2], body[3], body[4]]);
        let payload = &body[5..];
        let frame = match kind {
            KIND_HELLO => {
                if payload.len() < 2 {
                    return Err(invalid("a hello frame lacks its protocol version"));
                }
                Self::Hello {
                    protocol: u16::from_be_bytes([payload[0], payload[1]]),
                    version: String::from_utf8_lossy(&payload[2..]).into_owned(),
                }
            }
            KIND_OPEN => Self::Open {
                stream,
                channel: String::from_utf8(payload.to_vec())
                    .map_err(|_| invalid("a channel name is not UTF-8"))?,
            },
            KIND_DATA => Self::Data {
                stream,
                bytes: payload.to_vec(),
            },
            KIND_SHUTDOWN => Self::Shutdown { stream },
            KIND_CLOSE => Self::Close { stream },
            KIND_CREDIT => {
                let bytes: [u8; 4] = payload
                    .try_into()
                    .map_err(|_| invalid("a credit frame must carry four bytes"))?;
                Self::Credit {
                    stream,
                    bytes: u32::from_be_bytes(bytes),
                }
            }
            other => return Err(invalid(&format!("unknown frame kind {other}"))),
        };
        Ok(frame)
    }

    /// Reads one frame, or `None` at a clean end of input.
    pub fn read_from(reader: &mut impl Read) -> io::Result<Option<Self>> {
        let mut prefix = [0u8; 4];
        if !read_exact_or_eof(reader, &mut prefix)? {
            return Ok(None);
        }
        let length = checked_length(prefix)?;
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body)?;
        Self::decode(&body).map(Some)
    }
}

/// Validates a length prefix against [`MAX_FRAME_LEN`].
pub fn checked_length(prefix: [u8; 4]) -> io::Result<usize> {
    let length = u32::from_be_bytes(prefix) as usize;
    if !(5..=MAX_FRAME_LEN).contains(&length) {
        return Err(invalid(&format!(
            "a frame of {length} bytes is out of range"
        )));
    }
    Ok(length)
}

/// Whether `id` belongs to a stream the bridge opened.
pub fn is_bridge_stream(id: u32) -> bool {
    id % 2 == 1
}

fn read_exact_or_eof(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(frame: Frame) {
        let encoded = frame.encode();
        let mut reader = encoded.as_slice();
        assert_eq!(Frame::read_from(&mut reader).unwrap(), Some(frame));
        assert_eq!(Frame::read_from(&mut reader).unwrap(), None);
    }

    #[test]
    fn every_frame_kind_round_trips() {
        round_trip(Frame::hello());
        round_trip(Frame::Open {
            stream: 7,
            channel: "orchestrator".into(),
        });
        round_trip(Frame::Data {
            stream: 7,
            bytes: b"{\"id\":\"1\"}\n".to_vec(),
        });
        round_trip(Frame::Data {
            stream: 9,
            bytes: vec![0x10, 0x11, 0x00, 0xff],
        });
        round_trip(Frame::Shutdown { stream: 7 });
        round_trip(Frame::Close { stream: 7 });
        round_trip(Frame::Credit {
            stream: 7,
            bytes: STREAM_WINDOW,
        });
    }

    #[test]
    fn hello_carries_the_protocol_and_release_versions() {
        match Frame::hello() {
            Frame::Hello { protocol, version } => {
                assert_eq!(protocol, PROTOCOL_VERSION);
                assert_eq!(version, crate::VERSION);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn oversized_and_truncated_frames_are_rejected() {
        let oversized = ((MAX_FRAME_LEN + 1) as u32).to_be_bytes();
        assert!(Frame::read_from(&mut oversized.as_slice()).is_err());
        let mut truncated = Frame::Shutdown { stream: 1 }.encode();
        truncated.pop();
        assert!(Frame::read_from(&mut truncated.as_slice()).is_err());
        let mut unknown = Frame::Close { stream: 1 }.encode();
        unknown[4] = 42;
        assert!(Frame::read_from(&mut unknown.as_slice()).is_err());
    }

    #[test]
    fn bridge_streams_are_odd() {
        assert!(is_bridge_stream(1));
        assert!(!is_bridge_stream(2));
    }
}
