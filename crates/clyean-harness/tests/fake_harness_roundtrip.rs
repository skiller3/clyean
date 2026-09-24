// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

use std::sync::Arc;
use std::time::Duration;

use clyean_harness::session::BoxFuture;
use clyean_harness::{
    AgentSessionDriver, HarnessClient, HarnessSession, RpcFrame, TurnProgress, UiRequestHandler,
};
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::sync::mpsc;

async fn session() -> HarnessSession {
    let command = Command::new(env!("CARGO_BIN_EXE_clyean-fake-harness"));
    let client = HarnessClient::spawn(command, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(client.protocol_version(), 2);
    HarnessSession::new(client, Duration::from_secs(30))
}

#[tokio::test]
async fn prompt_streams_progress_and_returns_the_final_text() {
    let session = session().await;
    let (tx, mut rx) = mpsc::channel(64);
    let outcome = session
        .prompt("Please do the thing. REPLY: done with the thing".into(), tx)
        .await
        .unwrap();
    assert_eq!(outcome.assistant_text, "done with the thing");
    assert!(!outcome.failed());
    let mut deltas = String::new();
    let mut tools = Vec::new();
    while let Ok(progress) = rx.try_recv() {
        match progress {
            TurnProgress::TextDelta(text) => deltas.push_str(&text),
            TurnProgress::ToolStarted { tool_name } => tools.push(tool_name),
            TurnProgress::ToolFinished { .. } => {}
        }
    }
    assert_eq!(deltas, "Working on it");
    assert_eq!(tools, vec!["read"]);
    let info = session.session_info().await.unwrap();
    assert_eq!(info.session_id, "fake-session-id");
    assert_eq!(info.model.as_deref(), Some("fake/fake-model"));
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn continuing_agent_end_is_ignored_and_chunked_frames_are_reassembled() {
    let session = session().await;
    let (tx, _rx) = mpsc::channel(64);
    let outcome = session
        .prompt("RETRY_FIRST CHUNKED REPLY: reassembled".into(), tx)
        .await
        .unwrap();
    assert_eq!(outcome.assistant_text, "reassembled");
    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_commands_surface_as_errors() {
    let session = session().await;
    let error = session
        .client()
        .request("fail_please", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("scripted failure"));
    session.shutdown().await.unwrap();
}

struct Renewals;

impl UiRequestHandler for Renewals {
    fn answer<'a>(&'a self, request: &'a RpcFrame) -> BoxFuture<'a, Option<Value>> {
        Box::pin(async move {
            (request.0["title"] == "clyean:credentials").then(|| {
                json!({"value": format!("renewed {}", request.0["placeholder"].as_str().unwrap_or_default())})
            })
        })
    }
}

#[tokio::test]
async fn ui_requests_are_answered_by_the_handler_during_a_turn() {
    let command = Command::new(env!("CARGO_BIN_EXE_clyean-fake-harness"));
    let client = HarnessClient::spawn_with_ui_handler(
        command,
        Duration::from_secs(10),
        Some(Arc::new(Renewals)),
    )
    .await
    .unwrap();
    let session = HarnessSession::new(client, Duration::from_secs(30));
    let (tx, _rx) = mpsc::channel(64);
    let outcome = session.prompt("ASK: anthropic".into(), tx).await.unwrap();
    assert_eq!(outcome.assistant_text, "renewed anthropic");
    session.shutdown().await.unwrap();
}
