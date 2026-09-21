// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Project discovery, the `.clyean` scaffold layout, project configuration with
//! local overrides, project locking, and the change plan catalog.

pub mod config;
pub mod discovery;
pub mod identity;
pub mod ignore;
pub mod layout;
pub mod local_overlay;
pub mod lock;
pub mod plans;

pub use config::{ProjectConfig, ProjectType, SandboxConfig};
pub use discovery::ProjectDirectory;
pub use identity::ProjectId;
pub use layout::ProjectLayout;
pub use lock::{ProjectLock, ProjectLockError};
pub use plans::{PlanCatalog, PlanReference, PlanVersion};

/// Errors raised while reading or writing project state.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
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
    #[error(
        "workspace {workspace} must be the project directory {project} or one of its ancestors"
    )]
    WorkspaceNotAncestor {
        workspace: std::path::PathBuf,
        project: std::path::PathBuf,
    },
    #[error("project at {0} is not scaffolded (no .clyean/project.json)")]
    NotScaffolded(std::path::PathBuf),
    #[error("invalid change plan reference {0:?}")]
    InvalidPlanReference(String),
    #[error("change plan {0} does not exist")]
    PlanNotFound(String),
}

impl ProjectError {
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, ProjectError>;

/// The current UTC time formatted as RFC 3339 with second precision.
pub fn utc_now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("zero nanoseconds is valid")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formatting of a UTC timestamp cannot fail")
}
