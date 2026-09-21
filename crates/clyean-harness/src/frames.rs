// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Decoding of harness stdout lines into logical frames, including reassembly of the
//! protocol v2 `rpc_chunk` sequences that carry oversized objects.

use base64::Engine;
use serde_json::Value;

use crate::{HarnessError, Result};

/// One logical JSON object emitted by the harness.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcFrame(pub Value);

impl RpcFrame {
    pub fn kind(&self) -> &str {
        self.0
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    pub fn id(&self) -> Option<&str> {
        self.0.get("id").and_then(Value::as_str)
    }
}

#[derive(Debug, Default)]
struct ChunkAssembly {
    chunk_id: String,
    count: u64,
    byte_length: u64,
    next_index: u64,
    bytes: Vec<u8>,
}

/// Accumulates chunked frames until a whole object is available.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    assembly: Option<ChunkAssembly>,
    max_reassembled_bytes: u64,
}

impl FrameDecoder {
    pub fn new(max_reassembled_bytes: u64) -> Self {
        Self {
            assembly: None,
            max_reassembled_bytes,
        }
    }

    /// Feeds one stdout line.  Returns a frame when the line completes an object, or
    /// `None` while a chunk sequence is still in progress.
    pub fn feed_line(&mut self, line: &str) -> Result<Option<RpcFrame>> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let value: Value = serde_json::from_str(trimmed)
            .map_err(|e| HarnessError::MalformedFrame(format!("{e}: {}", truncate(trimmed))))?;
        if value.get("type").and_then(Value::as_str) != Some("rpc_chunk") {
            if self.assembly.is_some() {
                self.assembly = None;
                return Err(HarnessError::MalformedFrame(
                    "chunk sequence interrupted by another frame".into(),
                ));
            }
            return Ok(Some(RpcFrame(value)));
        }
        self.feed_chunk(&value)
    }

    fn feed_chunk(&mut self, value: &Value) -> Result<Option<RpcFrame>> {
        let chunk_id = string_field(value, "chunkId")?;
        let index = number_field(value, "index")?;
        let count = number_field(value, "count")?;
        let byte_length = number_field(value, "byteLength")?;
        let data = string_field(value, "data")?;
        if byte_length > self.max_reassembled_bytes {
            self.assembly = None;
            return Err(HarnessError::MalformedFrame(format!(
                "chunked frame of {byte_length} bytes exceeds the {} byte limit",
                self.max_reassembled_bytes
            )));
        }
        if index == 0 {
            self.assembly = Some(ChunkAssembly {
                chunk_id: chunk_id.clone(),
                count,
                byte_length,
                next_index: 0,
                bytes: Vec::with_capacity(byte_length as usize),
            });
        }
        let assembly = self.assembly.as_mut().ok_or_else(|| {
            HarnessError::MalformedFrame("chunk received without a sequence start".into())
        })?;
        if assembly.chunk_id != chunk_id || assembly.count != count || assembly.next_index != index
        {
            self.assembly = None;
            return Err(HarnessError::MalformedFrame(format!(
                "chunk {chunk_id}#{index} out of order"
            )));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|e| HarnessError::MalformedFrame(format!("chunk {chunk_id}#{index}: {e}")))?;
        assembly.bytes.extend_from_slice(&decoded);
        assembly.next_index += 1;
        if assembly.next_index < assembly.count {
            return Ok(None);
        }
        let assembly = self.assembly.take().expect("assembly present");
        if assembly.bytes.len() as u64 != assembly.byte_length {
            return Err(HarnessError::MalformedFrame(format!(
                "chunk {} reassembled to {} bytes, expected {}",
                assembly.chunk_id,
                assembly.bytes.len(),
                assembly.byte_length
            )));
        }
        let text = String::from_utf8(assembly.bytes).map_err(|e| {
            HarnessError::MalformedFrame(format!("chunk {}: {e}", assembly.chunk_id))
        })?;
        let value: Value = serde_json::from_str(&text).map_err(|e| {
            HarnessError::MalformedFrame(format!("chunk {}: {e}", assembly.chunk_id))
        })?;
        Ok(Some(RpcFrame(value)))
    }
}

fn string_field(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| HarnessError::MalformedFrame(format!("rpc_chunk without {field}")))
}

fn number_field(value: &Value, field: &str) -> Result<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| HarnessError::MalformedFrame(format!("rpc_chunk without numeric {field}")))
}

fn truncate(text: &str) -> String {
    text.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chunk(id: &str, index: u64, count: u64, total: usize, data: &[u8]) -> String {
        json!({
            "type": "rpc_chunk",
            "chunkId": id,
            "index": index,
            "count": count,
            "byteLength": total,
            "data": base64::engine::general_purpose::STANDARD.encode(data),
        })
        .to_string()
    }

    #[test]
    fn plain_frames_pass_through() {
        let mut decoder = FrameDecoder::new(1024);
        let frame = decoder
            .feed_line(r#"{"type":"ready","protocolVersion":1}"#)
            .unwrap()
            .unwrap();
        assert_eq!(frame.kind(), "ready");
        assert!(decoder.feed_line("   ").unwrap().is_none());
        assert!(decoder.feed_line("not json").is_err());
    }

    #[test]
    fn chunk_sequences_are_reassembled_in_order() {
        let payload = br#"{"type":"response","id":"x","success":true,"data":{"text":"hello"}}"#;
        let (first, second) = payload.split_at(20);
        let mut decoder = FrameDecoder::new(1024);
        assert!(decoder
            .feed_line(&chunk("c1", 0, 2, payload.len(), first))
            .unwrap()
            .is_none());
        let frame = decoder
            .feed_line(&chunk("c1", 1, 2, payload.len(), second))
            .unwrap()
            .unwrap();
        assert_eq!(frame.kind(), "response");
        assert_eq!(frame.0["data"]["text"], "hello");
    }

    #[test]
    fn interrupted_or_oversized_sequences_are_rejected() {
        let payload = b"{}";
        let mut decoder = FrameDecoder::new(1024);
        assert!(decoder
            .feed_line(&chunk("c1", 0, 2, 2, b"{"))
            .unwrap()
            .is_none());
        assert!(decoder.feed_line(r#"{"type":"agent_start"}"#).is_err());
        assert!(decoder.feed_line(&chunk("c2", 1, 2, 2, b"}")).is_err());
        assert!(decoder
            .feed_line(&chunk("c3", 0, 1, 4096, payload))
            .is_err());
    }
}
