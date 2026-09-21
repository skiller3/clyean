// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The host-side orchestrator: it serves the User Assistant over a Unix socket, runs the
//! planning, implementation, research, and scaffolding workflows by driving sub-agent
//! sessions, and journals every unit of work so that it can be resumed.

pub mod agents;
pub mod events;
pub mod journal;
pub mod prompts;
pub mod protocol;
pub mod scaffold;
pub mod server;
pub mod service;
pub mod verdict;
pub mod work;
pub mod workflows;

pub use agents::{AgentSessionFactory, SubAgentPool};
pub use journal::{Phase, WorkJournal, WorkKind, WorkStatus};
pub use protocol::{PromptType, Request, Response, StreamedEvent};
pub use service::{OrchestratorService, ProjectServices};
pub use work::{WorkId, WorkRegistry};

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error(transparent)]
    Project(#[from] clyean_project::ProjectError),
    #[error(transparent)]
    Agents(#[from] clyean_agents::AgentError),
    #[error(transparent)]
    Git(#[from] clyean_git::GitError),
    #[error(transparent)]
    Harness(#[from] clyean_harness::HarnessError),
    #[error(transparent)]
    Sandbox(#[from] clyean_sandbox::SandboxError),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid JSON: {source}")]
    Json {
        path: std::path::PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("the project is locked by another Clyean process")]
    ProjectLocked,
    #[error("the project is not scaffolded")]
    NotScaffolded,
    #[error("unknown work {0}")]
    WorkNotFound(String),
    #[error("work {work_id} has no pending information request {request_id}")]
    RequestNotFound { work_id: String, request_id: String },
    #[error("work was cancelled")]
    Cancelled,
    #[error("the {agent} agent did not return a usable verdict: {reason}")]
    Verdict { agent: String, reason: String },
    #[error("the {agent} agent's turn failed: {message}")]
    AgentTurnFailed { agent: String, message: String },
    #[error("{0}")]
    Workflow(String),
}

impl OrchestratorError {
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// The protocol error code of this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProjectLocked => "project_locked",
            Self::NotScaffolded => "not_scaffolded",
            Self::WorkNotFound(_) => "work_not_found",
            Self::RequestNotFound { .. } => "request_not_found",
            Self::Cancelled => "cancelled",
            _ => "internal",
        }
    }
}

pub type Result<T> = std::result::Result<T, OrchestratorError>;
