// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Turn-level driving of one harness session: send a prompt, stream its progress, and
//! collect the assistant's final text.  The driver trait lets orchestration logic run
//! against an in-process fake in tests.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::client::{HarnessClient, HarnessEvent};
use crate::{HarnessError, Result};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub session_id: String,
    pub session_file: Option<String>,
    pub model: Option<String>,
}

/// Incremental progress of one turn, suitable for relaying to a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnProgress {
    TextDelta(String),
    ToolStarted { tool_name: String },
    ToolFinished { tool_name: String, is_error: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnOutcome {
    pub assistant_text: String,
    pub stop_reason: Option<String>,
    pub error_message: Option<String>,
}

impl TurnOutcome {
    pub fn failed(&self) -> bool {
        self.stop_reason.as_deref() == Some("error") || self.error_message.is_some()
    }
}

/// Something that behaves like one agent session: it answers prompts and knows its
/// identity.  Implemented over RPC by [`HarnessSession`] and by fakes in tests.
pub trait AgentSessionDriver: Send + Sync {
    fn prompt<'a>(
        &'a self,
        text: String,
        progress: mpsc::Sender<TurnProgress>,
    ) -> BoxFuture<'a, Result<TurnOutcome>>;

    fn session_info<'a>(&'a self) -> BoxFuture<'a, Result<SessionInfo>>;

    fn shutdown<'a>(&'a self) -> BoxFuture<'a, Result<()>>;
}

pub struct HarnessSession {
    client: HarnessClient,
    turn_timeout: Duration,
}

impl HarnessSession {
    pub fn new(client: HarnessClient, turn_timeout: Duration) -> Self {
        Self {
            client,
            turn_timeout,
        }
    }

    pub fn client(&self) -> &HarnessClient {
        &self.client
    }

    async fn run_prompt(
        &self,
        text: String,
        progress: mpsc::Sender<TurnProgress>,
    ) -> Result<TurnOutcome> {
        let mut events = self.client.subscribe();
        let response = self
            .client
            .request(
                "prompt",
                json!({"message": text, "streamingBehavior": "followUp"}),
            )
            .await?;
        if response.0["data"]["agentInvoked"] == Value::Bool(false) {
            return Ok(TurnOutcome::default());
        }
        let outcome = tokio::time::timeout(self.turn_timeout, collect_turn(&mut events, &progress))
            .await
            .map_err(|_| HarnessError::Timeout("agent turn".into()))??;
        Ok(outcome)
    }
}

async fn collect_turn(
    events: &mut broadcast::Receiver<HarnessEvent>,
    progress: &mpsc::Sender<TurnProgress>,
) -> Result<TurnOutcome> {
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return Err(HarnessError::OutputClosed),
        };
        let frame = match event {
            HarnessEvent::Frame(frame) => frame,
            HarnessEvent::Closed => return Err(HarnessError::OutputClosed),
        };
        match frame.kind() {
            "message_update" => {
                let delta = &frame.0["assistantMessageEvent"];
                if delta["type"] == "text_delta" {
                    if let Some(text) = delta["delta"].as_str() {
                        let _ = progress
                            .send(TurnProgress::TextDelta(text.to_string()))
                            .await;
                    }
                }
            }
            "tool_execution_start" => {
                let _ = progress
                    .send(TurnProgress::ToolStarted {
                        tool_name: frame.0["toolName"].as_str().unwrap_or_default().to_string(),
                    })
                    .await;
            }
            "tool_execution_end" => {
                let _ = progress
                    .send(TurnProgress::ToolFinished {
                        tool_name: frame.0["toolName"].as_str().unwrap_or_default().to_string(),
                        is_error: frame.0["isError"].as_bool().unwrap_or(false),
                    })
                    .await;
            }
            "agent_end" => {
                if frame.0["willContinue"] == Value::Bool(true) {
                    continue;
                }
                return Ok(outcome_from_messages(&frame.0["messages"]));
            }
            "prompt_result" if frame.0["agentInvoked"] == Value::Bool(false) => {
                return Ok(TurnOutcome::default());
            }
            _ => {}
        }
    }
}

/// Extracts the final assistant message of a turn from `agent_end.messages`.
pub fn outcome_from_messages(messages: &Value) -> TurnOutcome {
    let Some(messages) = messages.as_array() else {
        return TurnOutcome::default();
    };
    let Some(assistant) = messages.iter().rev().find(|m| m["role"] == "assistant") else {
        return TurnOutcome::default();
    };
    let assistant_text = assistant["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    TurnOutcome {
        assistant_text,
        stop_reason: assistant["stopReason"].as_str().map(str::to_string),
        error_message: assistant["errorMessage"].as_str().map(str::to_string),
    }
}

impl AgentSessionDriver for HarnessSession {
    fn prompt<'a>(
        &'a self,
        text: String,
        progress: mpsc::Sender<TurnProgress>,
    ) -> BoxFuture<'a, Result<TurnOutcome>> {
        Box::pin(self.run_prompt(text, progress))
    }

    fn session_info<'a>(&'a self) -> BoxFuture<'a, Result<SessionInfo>> {
        Box::pin(async move {
            let state = self.client.request("get_state", json!({})).await?;
            let data = &state.0["data"];
            let model = match (
                data["model"]["provider"].as_str(),
                data["model"]["id"].as_str(),
            ) {
                (Some(provider), Some(id)) => Some(format!("{provider}/{id}")),
                _ => None,
            };
            Ok(SessionInfo {
                session_id: data["sessionId"].as_str().unwrap_or_default().to_string(),
                session_file: data["sessionFile"].as_str().map(str::to_string),
                model,
            })
        })
    }

    fn shutdown<'a>(&'a self) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.client.shutdown(Duration::from_secs(10)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_takes_the_last_assistant_text_blocks() {
        let messages = json!([
            {"role": "user", "content": [{"type": "text", "text": "hi"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "first"}], "stopReason": "stop"},
            {"role": "toolResult", "content": []},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "..."},
                {"type": "text", "text": "final "},
                {"type": "text", "text": "answer"}
            ], "stopReason": "stop"}
        ]);
        let outcome = outcome_from_messages(&messages);
        assert_eq!(outcome.assistant_text, "final answer");
        assert_eq!(outcome.stop_reason.as_deref(), Some("stop"));
        assert!(!outcome.failed());
        let failed = outcome_from_messages(&json!([
            {"role": "assistant", "content": [], "stopReason": "error", "errorMessage": "overloaded"}
        ]));
        assert!(failed.failed());
    }
}
