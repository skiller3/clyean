// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Podman-based sandboxing of Clyean agents: the shared root filesystem under
//! `.clyean/container-root`, its provisioning, the mounts and environment of every agent
//! container, and the Herdr socket pass-through for the User Assistant.

pub mod container;
pub mod herdr;
pub mod launch;
pub mod podman;
pub mod provisioning;
pub mod rootfs;
pub mod user;

pub use container::{AgentContainerSpec, ContainerPaths, MountSpec};
pub use herdr::HerdrHostContext;
pub use launch::{LaunchContext, LaunchRole, SandboxRunner};
pub use podman::{Podman, PodmanHostInfo};
pub use user::ContainerUser;

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("podman is not installed or not on PATH ({0}); install Podman (https://podman.io/) and try again")]
    PodmanMissing(std::io::Error),
    #[error("podman {command} failed: {stderr}")]
    PodmanFailed { command: String, stderr: String },
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("download of {url} failed: {reason}")]
    Download { url: String, reason: String },
    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    Checksum {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("the sandbox root filesystem is not provisioned: {0}")]
    NotProvisioned(String),
    #[error("harness binary not found: {0}")]
    HarnessMissing(String),
    #[error("{0}")]
    Invalid(String),
}

impl SandboxError {
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, SandboxError>;
