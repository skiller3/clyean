// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The frames that travel over the bridge session.  Every frame is a big-endian `u32`
//! length (of everything after it), a `u8` kind, a `u32` stream id, and a payload.
//!
//! Streams opened by the bridge (connections accepted inside the container) have odd ids;
//! streams opened by the host (relays into the container) have even ids.  Each direction
//! of a stream has a window of [`STREAM_WINDOW`] bytes: a side sends data only while it
//! holds credit, and the receiver returns credit as it delivers the data, so a slow reader
//! never makes the other side buffer without bound.
//!
//! A relay stream carries TCP bytes as they are, and UDP and SCTP traffic as [`Message`]s,
//! so that datagram and message boundaries survive the byte stream.

use std::io::{self, Read};

/// Bumped whenever the frame format or its rules change; both ends must agree.
pub const PROTOCOL_VERSION: u16 = 2;

/// The largest data payload in one frame.
pub const MAX_DATA: usize = 64 * 1024;

/// The credit each direction of a stream starts with.
pub const STREAM_WINDOW: u32 = 256 * 1024;

/// The most streams the bridge keeps open at once; further connections are refused.
pub const MAX_STREAMS: usize = 256;

/// The most streams the host may have open at once, which leaves the rest of
/// [`MAX_STREAMS`] to the bridge's own channels, so relays cannot starve them.
pub const MAX_HOST_STREAMS: usize = 224;

/// The largest frame either end accepts, which bounds the memory a single frame can take.
pub const MAX_FRAME_LEN: usize = 5 + MAX_DATA;

/// The largest payload one [`Message`] carries.
pub const MAX_MESSAGE: usize = 256 * 1024;

/// The encoded size of a [`Message`]'s header.
pub const MESSAGE_HEADER_LEN: usize = 10;

/// The longest command line an [`Frame::Open`] reports for the connecting process.
const MAX_PEER_COMMAND: usize = 512;

const KIND_HELLO: u8 = 1;
const KIND_OPEN: u8 = 2;
const KIND_DATA: u8 = 3;
const KIND_SHUTDOWN: u8 = 4;
const KIND_CLOSE: u8 = 5;
const KIND_CREDIT: u8 = 6;

/// The process on the far end of a connection the bridge accepted, as the bridge read it
/// with `SO_PEERCRED` and `/proc`.  The command line is whatever that process set, so it
/// is reported, not verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub pid: u32,
    pub command: String,
}

impl Peer {
    /// A peer whose command line is cut to [`MAX_PEER_COMMAND`] bytes at a character
    /// boundary.
    pub fn new(pid: u32, command: &str) -> Self {
        let mut end = command.len().min(MAX_PEER_COMMAND);
        while !command.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            pid,
            command: command[..end].to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// The bridge's first frame: the protocol it speaks and its release version.
    Hello { protocol: u16, version: String },
    /// A new stream on the named channel.  Streams the bridge opens name the process that
    /// connected, when the bridge could read it.
    Open {
        stream: u32,
        channel: String,
        peer: Option<Peer>,
    },
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
            Self::Open {
                stream,
                channel,
                peer,
            } => (KIND_OPEN, *stream, encode_open(channel, peer.as_ref())),
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
            KIND_OPEN => {
                let (channel, peer) = decode_open(payload)?;
                Self::Open {
                    stream,
                    channel,
                    peer,
                }
            }
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

/// An open frame's payload: the channel name, then, for a known peer, a NUL, its pid in
/// decimal, a NUL, and its command line.  Channel names never contain a NUL.
fn encode_open(channel: &str, peer: Option<&Peer>) -> Vec<u8> {
    let mut payload = channel.as_bytes().to_vec();
    if let Some(peer) = peer {
        payload.push(0);
        payload.extend_from_slice(peer.pid.to_string().as_bytes());
        payload.push(0);
        payload.extend_from_slice(peer.command.as_bytes());
    }
    payload
}

fn decode_open(payload: &[u8]) -> io::Result<(String, Option<Peer>)> {
    let mut parts = payload.splitn(3, |byte| *byte == 0);
    let channel = String::from_utf8(parts.next().unwrap_or_default().to_vec())
        .map_err(|_| invalid("a channel name is not UTF-8"))?;
    let peer = match (parts.next(), parts.next()) {
        (Some(pid), Some(command)) => {
            let pid = std::str::from_utf8(pid)
                .ok()
                .and_then(|pid| pid.parse().ok())
                .ok_or_else(|| invalid("an open frame's peer pid is not a number"))?;
            Some(Peer::new(pid, &String::from_utf8_lossy(command)))
        }
        (None, _) => None,
        (Some(_), None) => return Err(invalid("an open frame's peer lacks its command line")),
    };
    Ok((channel, peer))
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

/// The transport protocol a relay carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RelayProtocol {
    Tcp,
    Udp,
    Sctp,
}

impl RelayProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Sctp => "sctp",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "tcp" => Some(Self::Tcp),
            "udp" => Some(Self::Udp),
            "sctp" => Some(Self::Sctp),
            _ => None,
        }
    }

    /// Whether streams of this protocol carry [`Message`]s rather than bytes.
    pub fn carries_messages(self) -> bool {
        !matches!(self, Self::Tcp)
    }
}

/// Where a relay stream leads inside the container: a protocol and a port on the
/// container's loopback, named by the channel `relay:<protocol>:<port>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayTarget {
    pub protocol: RelayProtocol,
    pub port: u16,
}

impl RelayTarget {
    pub fn channel(&self) -> String {
        format!("relay:{}:{}", self.protocol.as_str(), self.port)
    }

    /// The target a channel names, or `None` for any other channel.
    pub fn from_channel(channel: &str) -> Option<Self> {
        let rest = channel.strip_prefix("relay:")?;
        let (protocol, port) = rest.split_once(':')?;
        let port: u16 = port.parse().ok()?;
        if port == 0 {
            return None;
        }
        Some(Self {
            protocol: RelayProtocol::parse(protocol)?,
            port,
        })
    }
}

/// One UDP datagram or SCTP message on a relay stream: a big-endian `u32` payload length,
/// a `u16` SCTP stream number, a `u32` payload protocol identifier (both zero for UDP),
/// and the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub stream_number: u16,
    pub payload_protocol: u32,
    pub payload: Vec<u8>,
}

impl Message {
    pub fn datagram(payload: Vec<u8>) -> Self {
        Self {
            stream_number: 0,
            payload_protocol: 0,
            payload,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(MESSAGE_HEADER_LEN + self.payload.len());
        encoded.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        encoded.extend_from_slice(&self.stream_number.to_be_bytes());
        encoded.extend_from_slice(&self.payload_protocol.to_be_bytes());
        encoded.extend_from_slice(&self.payload);
        encoded
    }
}

/// Reassembles [`Message`]s from the bytes of a relay stream, which may split or join
/// them arbitrarily.
#[derive(Debug, Default)]
pub struct MessageReader {
    buffer: Vec<u8>,
}

impl MessageReader {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next complete message, `None` while one is incomplete, or an error for a
    /// message longer than [`MAX_MESSAGE`].
    pub fn next_message(&mut self) -> io::Result<Option<Message>> {
        if self.buffer.len() < MESSAGE_HEADER_LEN {
            return Ok(None);
        }
        let length = u32::from_be_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;
        if length > MAX_MESSAGE {
            return Err(invalid(&format!(
                "a message of {length} bytes is longer than {MAX_MESSAGE}"
            )));
        }
        if self.buffer.len() < MESSAGE_HEADER_LEN + length {
            return Ok(None);
        }
        let stream_number = u16::from_be_bytes([self.buffer[4], self.buffer[5]]);
        let payload_protocol = u32::from_be_bytes([
            self.buffer[6],
            self.buffer[7],
            self.buffer[8],
            self.buffer[9],
        ]);
        let payload = self.buffer[MESSAGE_HEADER_LEN..MESSAGE_HEADER_LEN + length].to_vec();
        self.buffer.drain(..MESSAGE_HEADER_LEN + length);
        Ok(Some(Message {
            stream_number,
            payload_protocol,
            payload,
        }))
    }
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
            peer: None,
        });
        round_trip(Frame::Open {
            stream: 9,
            channel: "orchestrator".into(),
            peer: Some(Peer::new(4242, "python3 -m http.server 8000")),
        });
        round_trip(Frame::Open {
            stream: 2,
            channel: "relay:udp:5353".into(),
            peer: None,
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
    fn a_peer_command_line_is_cut_at_a_character_boundary() {
        let long = "é".repeat(400);
        let peer = Peer::new(1, &long);
        assert!(peer.command.len() <= 512);
        assert!(peer.command.chars().all(|c| c == 'é'));
        let mut malformed = Frame::Open {
            stream: 1,
            channel: "orchestrator".into(),
            peer: Some(Peer::new(3, "x")),
        }
        .encode();
        let pid_start = malformed.len() - 3;
        malformed[pid_start] = b'z';
        assert!(Frame::read_from(&mut malformed.as_slice()).is_err());
    }

    #[test]
    fn bridge_streams_are_odd() {
        assert!(is_bridge_stream(1));
        assert!(!is_bridge_stream(2));
    }

    #[test]
    fn relay_channels_name_a_protocol_and_a_port() {
        let target = RelayTarget {
            protocol: RelayProtocol::Udp,
            port: 5353,
        };
        assert_eq!(target.channel(), "relay:udp:5353");
        assert_eq!(RelayTarget::from_channel("relay:udp:5353"), Some(target));
        assert_eq!(
            RelayTarget::from_channel("relay:sctp:36412").map(|t| t.protocol),
            Some(RelayProtocol::Sctp)
        );
        for invalid in [
            "relay:54549",
            "relay:tcp:0",
            "relay:tcp:65536",
            "relay:icmp:1",
            "orchestrator",
            "relay:tcp:",
        ] {
            assert_eq!(RelayTarget::from_channel(invalid), None, "{invalid}");
        }
        assert!(RelayProtocol::Udp.carries_messages());
        assert!(!RelayProtocol::Tcp.carries_messages());
    }

    #[test]
    fn messages_survive_being_split_and_joined() {
        let first = Message {
            stream_number: 3,
            payload_protocol: 46,
            payload: b"hello".to_vec(),
        };
        let second = Message::datagram(vec![7u8; 3000]);
        let mut bytes = first.encode();
        bytes.extend(second.encode());
        let mut reader = MessageReader::default();
        let mut messages = Vec::new();
        for chunk in bytes.chunks(7) {
            reader.push(chunk);
            while let Some(message) = reader.next_message().unwrap() {
                messages.push(message);
            }
        }
        assert_eq!(messages, vec![first, second]);
        let mut oversized = MessageReader::default();
        oversized.push(&((MAX_MESSAGE + 1) as u32).to_be_bytes());
        oversized.push(&[0u8; 6]);
        assert!(oversized.next_message().is_err());
    }
}
