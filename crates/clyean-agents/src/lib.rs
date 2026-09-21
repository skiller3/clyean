// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The roster of Clyean agents, their baseline instructions and harness settings, and the
//! projection of both into per-agent harness profiles inside the sandbox.

pub mod extensions;
pub mod instructions;
pub mod profile;
pub mod roster;
pub mod settings;

pub use extensions::managed_extensions;
pub use profile::{ManagedExtension, ProfileProjection};
pub use roster::{AgentId, AgentStatus};

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
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
    #[error("agent {0} is a placeholder and has no baseline instructions yet")]
    NotImplemented(&'static str),
}

impl AgentError {
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, AgentError>;
