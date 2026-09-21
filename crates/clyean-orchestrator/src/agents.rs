// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Sub-agent sessions of one unit of work.  Every agent gets a fresh session per unit of
//! work, and that session is reused for every step of the work, including across the
//! collection of follow-up information from the user.

use std::collections::HashMap;
use std::sync::Arc;

use clyean_agents::AgentId;
use clyean_harness::session::BoxFuture;
use clyean_harness::{AgentSessionDriver, TurnOutcome, TurnProgress};
use tokio::sync::{mpsc, Mutex};

use crate::journal::{SessionRecord, WorkJournal};
use crate::{OrchestratorError, Result};

/// Opens sessions for sub-agents.  The production implementation launches a harness in
/// a sandbox container; tests substitute scripted fakes.
pub trait AgentSessionFactory: Send + Sync {
    fn open<'a>(
        &'a self,
        agent: AgentId,
        work_id: &'a str,
        resume_session_file: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Box<dyn AgentSessionDriver>>>;
}

/// Lazily opened, reused sessions of the agents participating in one unit of work.
pub struct SubAgentPool {
    factory: Arc<dyn AgentSessionFactory>,
    work_id: String,
    sessions: Mutex<HashMap<AgentId, Arc<Box<dyn AgentSessionDriver>>>>,
}

impl SubAgentPool {
    pub fn new(factory: Arc<dyn AgentSessionFactory>, work_id: impl Into<String>) -> Self {
        Self {
            factory,
            work_id: work_id.into(),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// The session of `agent`, opened on first use.  When the journal records a session
    /// file for the agent (from an interrupted run), that session is resumed.
    pub async fn session(
        &self,
        agent: AgentId,
        journal: &mut WorkJournal,
    ) -> Result<Arc<Box<dyn AgentSessionDriver>>> {
        if let Some(existing) = self.sessions.lock().await.get(&agent) {
            return Ok(existing.clone());
        }
        let resume = journal
            .sessions
            .get(agent.id())
            .and_then(|record| record.session_file.clone());
        let session = self
            .factory
            .open(agent, &self.work_id, resume.as_deref())
            .await?;
        let session = Arc::new(session);
        if let Ok(info) = session.session_info().await {
            journal.sessions.insert(
                agent.id().to_string(),
                SessionRecord {
                    session_id: info.session_id,
                    session_file: info.session_file,
                },
            );
        }
        self.sessions.lock().await.insert(agent, session.clone());
        Ok(session)
    }

    /// Sends one prompt to `agent` and returns the turn's outcome, failing when the turn
    /// itself failed (for example on a provider error).
    pub async fn prompt(
        &self,
        agent: AgentId,
        journal: &mut WorkJournal,
        text: String,
        progress: mpsc::Sender<TurnProgress>,
    ) -> Result<TurnOutcome> {
        let session = self.session(agent, journal).await?;
        let outcome = session.prompt(text, progress).await?;
        if outcome.failed() {
            return Err(OrchestratorError::AgentTurnFailed {
                agent: agent.id().to_string(),
                message: outcome
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "the model stopped with an error".to_string()),
            });
        }
        Ok(outcome)
    }

    pub async fn shutdown_all(&self) {
        let sessions: Vec<_> = self.sessions.lock().await.drain().map(|(_, s)| s).collect();
        for session in sessions {
            if let Err(error) = session.shutdown().await {
                tracing::warn!(target: "clyean::orchestrator", %error, "failed to shut down a sub-agent session");
            }
        }
    }
}

/// A scripted fake for tests: each agent answers prompts from a queue of replies and
/// records every prompt it received.
pub mod fake {
    use super::*;
    use clyean_harness::SessionInfo;
    use std::collections::VecDeque;

    pub type PromptLog = Arc<Mutex<Vec<(AgentId, String)>>>;
    pub type OpenLog = Arc<Mutex<Vec<(AgentId, Option<String>)>>>;

    #[derive(Default)]
    pub struct ScriptedFactory {
        replies: Mutex<HashMap<AgentId, VecDeque<String>>>,
        pub prompts: PromptLog,
        pub opened: OpenLog,
    }

    impl ScriptedFactory {
        pub fn new() -> Self {
            Self::default()
        }

        pub async fn script(
            &self,
            agent: AgentId,
            replies: impl IntoIterator<Item = impl Into<String>>,
        ) {
            self.replies
                .lock()
                .await
                .entry(agent)
                .or_default()
                .extend(replies.into_iter().map(Into::into));
        }

        pub async fn prompts_to(&self, agent: AgentId) -> Vec<String> {
            self.prompts
                .lock()
                .await
                .iter()
                .filter(|(a, _)| *a == agent)
                .map(|(_, p)| p.clone())
                .collect()
        }
    }

    struct ScriptedSession {
        agent: AgentId,
        replies: Arc<Mutex<VecDeque<String>>>,
        prompts: PromptLog,
    }

    impl AgentSessionDriver for ScriptedSession {
        fn prompt<'a>(
            &'a self,
            text: String,
            progress: mpsc::Sender<TurnProgress>,
        ) -> BoxFuture<'a, clyean_harness::Result<TurnOutcome>> {
            Box::pin(async move {
                self.prompts.lock().await.push((self.agent, text));
                let reply = self.replies.lock().await.pop_front().unwrap_or_else(|| {
                    "no scripted reply\n```json\n{\"decision\": \"unscripted\"}\n```".to_string()
                });
                let _ = progress
                    .send(TurnProgress::TextDelta(format!("{reply}\n")))
                    .await;
                Ok(TurnOutcome {
                    assistant_text: reply,
                    stop_reason: Some("stop".into()),
                    error_message: None,
                })
            })
        }

        fn session_info<'a>(&'a self) -> BoxFuture<'a, clyean_harness::Result<SessionInfo>> {
            Box::pin(async move {
                Ok(SessionInfo {
                    session_id: format!("fake-{}", self.agent.id()),
                    session_file: Some(format!("/fake/sessions/{}.jsonl", self.agent.id())),
                    model: Some("fake/model".into()),
                })
            })
        }

        fn shutdown<'a>(&'a self) -> BoxFuture<'a, clyean_harness::Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl AgentSessionFactory for ScriptedFactory {
        fn open<'a>(
            &'a self,
            agent: AgentId,
            _work_id: &'a str,
            resume_session_file: Option<&'a str>,
        ) -> BoxFuture<'a, Result<Box<dyn AgentSessionDriver>>> {
            Box::pin(async move {
                self.opened
                    .lock()
                    .await
                    .push((agent, resume_session_file.map(str::to_string)));
                let replies = self.replies.lock().await.remove(&agent).unwrap_or_default();
                Ok(Box::new(ScriptedSession {
                    agent,
                    replies: Arc::new(Mutex::new(replies)),
                    prompts: self.prompts.clone(),
                }) as Box<dyn AgentSessionDriver>)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::ScriptedFactory;
    use super::*;
    use crate::journal::WorkKind;

    #[tokio::test]
    async fn sessions_are_reused_and_recorded_in_the_journal() {
        let factory = Arc::new(ScriptedFactory::new());
        factory
            .script(AgentId::Specifier, ["first", "second"])
            .await;
        let pool = SubAgentPool::new(factory.clone(), "w1");
        let mut journal = WorkJournal::new("w1".into(), WorkKind::Planning, "p".into());
        let (tx, _rx) = mpsc::channel(8);
        let first = pool
            .prompt(AgentId::Specifier, &mut journal, "one".into(), tx.clone())
            .await
            .unwrap();
        let second = pool
            .prompt(AgentId::Specifier, &mut journal, "two".into(), tx)
            .await
            .unwrap();
        assert_eq!(first.assistant_text, "first");
        assert_eq!(second.assistant_text, "second");
        assert_eq!(factory.opened.lock().await.len(), 1);
        assert_eq!(
            journal.sessions["specifier"].session_file.as_deref(),
            Some("/fake/sessions/specifier.jsonl")
        );
        pool.shutdown_all().await;
    }

    #[tokio::test]
    async fn recorded_sessions_are_resumed_by_a_new_pool() {
        let factory = Arc::new(ScriptedFactory::new());
        let pool = SubAgentPool::new(factory.clone(), "w1");
        let mut journal = WorkJournal::new("w1".into(), WorkKind::Planning, "p".into());
        journal.sessions.insert(
            "programmer".into(),
            SessionRecord {
                session_id: "old".into(),
                session_file: Some("/old/session.jsonl".into()),
            },
        );
        pool.session(AgentId::Programmer, &mut journal)
            .await
            .unwrap();
        assert_eq!(
            factory.opened.lock().await[0],
            (AgentId::Programmer, Some("/old/session.jsonl".to_string()))
        );
    }
}
