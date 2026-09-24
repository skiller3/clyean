// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! A stand-in for the contained harness's RPC mode, used by integration tests.  It speaks
//! the same newline-delimited JSON protocol: a ready frame, protocol negotiation, and a
//! scripted agent turn for every `prompt` command.  A prompt containing the marker
//! `REPLY:` is answered with the text after the marker; any other prompt is echoed.
//! A prompt containing `CHUNKED` answers through protocol v2 chunk frames.  A prompt
//! containing `ASK:` first sends an extension UI input request whose placeholder is the
//! text after the marker, and answers with the value it receives.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    emit(
        &mut stdout,
        &json!({
            "type": "ready",
            "protocolVersion": 1,
            "supportedProtocolVersions": [1, 2],
            "maxFrameBytes": 1048576,
            "maxReassembledFrameBytes": 67108864
        }),
    );
    let mut chunked_allowed = false;
    let mut awaiting_answer = false;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(command): Result<Value, _> = serde_json::from_str(&line) else {
            emit(
                &mut stdout,
                &json!({"type": "response", "command": "parse", "success": false, "error": "malformed"}),
            );
            continue;
        };
        let id = command["id"].clone();
        match command["type"].as_str().unwrap_or_default() {
            "negotiate_protocol" => {
                chunked_allowed = true;
                emit(
                    &mut stdout,
                    &json!({"id": id, "type": "response", "command": "negotiate_protocol", "success": true}),
                );
            }
            "prompt" => {
                let message = command["message"].as_str().unwrap_or_default().to_string();
                emit(
                    &mut stdout,
                    &json!({"id": id, "type": "response", "command": "prompt", "success": true, "data": {"agentInvoked": true}}),
                );
                match message.split_once("ASK:") {
                    Some((_, question)) => {
                        emit(
                            &mut stdout,
                            &json!({"type": "extension_ui_request", "id": "ui-1", "method": "input", "title": "clyean:credentials", "placeholder": question.trim(), "timeout": 60000}),
                        );
                        awaiting_answer = true;
                    }
                    None => scripted_turn(&mut stdout, &message, chunked_allowed),
                }
            }
            "extension_ui_response" if awaiting_answer && id == "ui-1" => {
                awaiting_answer = false;
                let answer = command["value"].as_str().unwrap_or("<cancelled>");
                scripted_turn(&mut stdout, &format!("REPLY: {answer}"), chunked_allowed);
            }
            "get_state" => emit(
                &mut stdout,
                &json!({"id": id, "type": "response", "command": "get_state", "success": true, "data": {
                    "sessionId": "fake-session-id",
                    "sessionFile": "/home/fake/.omp/profiles/programmer/agent/sessions/fake.jsonl",
                    "model": {"provider": "fake", "id": "fake-model"}
                }}),
            ),
            "fail_please" => emit(
                &mut stdout,
                &json!({"id": id, "type": "response", "command": "fail_please", "success": false, "error": "scripted failure"}),
            ),
            other => emit(
                &mut stdout,
                &json!({"id": id, "type": "response", "command": other, "success": true}),
            ),
        }
    }
}

fn scripted_turn(stdout: &mut std::io::Stdout, message: &str, chunked_allowed: bool) {
    let reply = match message.split_once("REPLY:") {
        Some((_, reply)) => reply.trim().to_string(),
        None => format!("Echo: {message}"),
    };
    emit(stdout, &json!({"type": "agent_start"}));
    for word in ["Working", " on", " it"] {
        emit(
            stdout,
            &json!({"type": "message_update", "message": {"role": "assistant"}, "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": word}}),
        );
    }
    emit(
        stdout,
        &json!({"type": "tool_execution_start", "toolCallId": "t1", "toolName": "read", "args": {}}),
    );
    emit(
        stdout,
        &json!({"type": "tool_execution_end", "toolCallId": "t1", "toolName": "read", "result": {}, "isError": false}),
    );
    if message.contains("RETRY_FIRST") {
        emit(
            stdout,
            &json!({"type": "agent_end", "willContinue": true, "messages": []}),
        );
    }
    let end = json!({"type": "agent_end", "messages": [
        {"role": "user", "content": [{"type": "text", "text": message}]},
        {"role": "assistant", "content": [{"type": "text", "text": reply}], "stopReason": "stop"}
    ]});
    if chunked_allowed && message.contains("CHUNKED") {
        emit_chunked(stdout, &end);
    } else {
        emit(stdout, &end);
    }
}

fn emit(stdout: &mut std::io::Stdout, value: &Value) {
    let mut line = value.to_string();
    line.push('\n');
    let _ = stdout.write_all(line.as_bytes());
    let _ = stdout.flush();
}

fn emit_chunked(stdout: &mut std::io::Stdout, value: &Value) {
    use base64::Engine;
    let bytes = value.to_string().into_bytes();
    let chunks: Vec<&[u8]> = bytes.chunks(48).collect();
    for (index, chunk) in chunks.iter().enumerate() {
        emit(
            stdout,
            &json!({
                "type": "rpc_chunk",
                "chunkId": "fake-chunk-1",
                "index": index,
                "count": chunks.len(),
                "byteLength": bytes.len(),
                "data": base64::engine::general_purpose::STANDARD.encode(chunk),
            }),
        );
    }
}
