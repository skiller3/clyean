// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Publication of progress to whoever is streaming a unit of work, with sub-agent text
//! coalesced into lines so that the socket carries readable chunks.

use clyean_harness::TurnProgress;
use tokio::sync::mpsc;

use crate::journal::Phase;
use crate::protocol::StreamedEvent;
use crate::work::WorkHandle;

#[derive(Debug, Clone)]
pub struct EventSink {
    handle: WorkHandle,
}

impl EventSink {
    pub fn new(handle: WorkHandle) -> Self {
        Self { handle }
    }

    pub fn work_id(&self) -> &str {
        self.handle.work_id.as_str()
    }

    pub async fn progress(&self, agent: &str, phase: Phase, text: impl Into<String>) {
        let text = text.into();
        tracing::info!(target: "clyean::orchestrator", agent, phase = phase.label(), %text);
        self.handle
            .publish(StreamedEvent::Progress {
                work_id: self.work_id().to_string(),
                seq: self.handle.next_sequence(),
                agent: agent.to_string(),
                phase: phase.label().to_string(),
                text,
            })
            .await;
    }

    pub async fn agent_output(&self, agent: &str, phase: Phase, text: String) {
        self.handle
            .publish(StreamedEvent::AgentOutput {
                work_id: self.work_id().to_string(),
                seq: self.handle.next_sequence(),
                agent: agent.to_string(),
                phase: phase.label().to_string(),
                text,
            })
            .await;
    }

    pub async fn information_requested(
        &self,
        request_id: &str,
        questions: &[String],
        context: &str,
    ) {
        self.handle
            .publish(StreamedEvent::InformationRequested {
                work_id: self.work_id().to_string(),
                seq: self.handle.next_sequence(),
                request_id: request_id.to_string(),
                questions: questions.to_vec(),
                context: context.to_string(),
            })
            .await;
    }

    pub async fn completed(&self, summary: &str, artifacts: &[String], plan: Option<&str>) {
        self.handle
            .publish(StreamedEvent::Completed {
                work_id: self.work_id().to_string(),
                seq: self.handle.next_sequence(),
                summary: summary.to_string(),
                artifacts: artifacts.to_vec(),
                plan: plan.map(str::to_string),
            })
            .await;
    }

    pub async fn failed(&self, code: &str, message: &str) {
        self.handle
            .publish(StreamedEvent::Failed {
                work_id: self.work_id().to_string(),
                seq: self.handle.next_sequence(),
                code: code.to_string(),
                message: message.to_string(),
            })
            .await;
    }

    /// Creates a progress channel whose receiving side turns turn progress into
    /// `agent_output` events, flushing text at line boundaries.  The returned relay
    /// completes once the sender is dropped and every buffered line is published; await
    /// it before publishing anything that must follow the agent's output.
    pub fn turn_progress_channel(
        &self,
        agent: &str,
        phase: Phase,
    ) -> (mpsc::Sender<TurnProgress>, ProgressRelay) {
        let (tx, mut rx) = mpsc::channel::<TurnProgress>(256);
        let sink = self.clone();
        let agent = agent.to_string();
        let relay = tokio::spawn(async move {
            let mut buffer = String::new();
            while let Some(progress) = rx.recv().await {
                match progress {
                    TurnProgress::TextDelta(delta) => {
                        buffer.push_str(&delta);
                        if let Some(index) = buffer.rfind('\n') {
                            let line = buffer[..=index].to_string();
                            buffer.drain(..=index);
                            sink.agent_output(&agent, phase, line).await;
                        }
                    }
                    TurnProgress::ToolStarted { tool_name } => {
                        flush(&sink, &agent, phase, &mut buffer).await;
                        sink.progress(&agent, phase, format!("using tool {tool_name}"))
                            .await;
                    }
                    TurnProgress::ToolFinished {
                        tool_name,
                        is_error,
                    } => {
                        if is_error {
                            sink.progress(
                                &agent,
                                phase,
                                format!("tool {tool_name} reported an error"),
                            )
                            .await;
                        }
                    }
                }
            }
            flush(&sink, &agent, phase, &mut buffer).await;
        });
        (tx, ProgressRelay(relay))
    }
}

/// Completion handle of a progress relay task.
pub struct ProgressRelay(tokio::task::JoinHandle<()>);

impl ProgressRelay {
    pub async fn finish(self) {
        let _ = self.0.await;
    }
}

async fn flush(sink: &EventSink, agent: &str, phase: Phase, buffer: &mut String) {
    if !buffer.trim().is_empty() {
        sink.agent_output(agent, phase, std::mem::take(buffer))
            .await;
    } else {
        buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::WorkId;

    #[tokio::test]
    async fn text_deltas_are_coalesced_into_lines() {
        let (handle, _rx) = WorkHandle::new(WorkId::from_string("w"));
        let mut events = handle.subscribe();
        let sink = EventSink::new(handle);
        let (tx, relay) =
            sink.turn_progress_channel("programmer", Phase::ImplementationProgramming);
        tx.send(TurnProgress::TextDelta("Hel".into()))
            .await
            .unwrap();
        tx.send(TurnProgress::TextDelta("lo\nwor".into()))
            .await
            .unwrap();
        tx.send(TurnProgress::ToolStarted {
            tool_name: "bash".into(),
        })
        .await
        .unwrap();
        drop(tx);
        relay.finish().await;
        let first = events.recv().await.unwrap();
        assert!(matches!(first, StreamedEvent::AgentOutput { ref text, .. } if text == "Hello\n"));
        let second = events.recv().await.unwrap();
        assert!(matches!(second, StreamedEvent::AgentOutput { ref text, .. } if text == "wor"));
        let third = events.recv().await.unwrap();
        assert!(
            matches!(third, StreamedEvent::Progress { ref text, .. } if text == "using tool bash")
        );
    }
}
