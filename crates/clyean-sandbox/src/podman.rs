// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! A small wrapper over the `podman` command line.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::{Result, SandboxError};

#[derive(Debug, Clone)]
pub struct Podman {
    binary: PathBuf,
}

impl Default for Podman {
    fn default() -> Self {
        Self::new("podman")
    }
}

impl Podman {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    pub fn binary(&self) -> &std::path::Path {
        &self.binary
    }

    /// Runs podman with `args`, returning stdout on success.
    pub fn output<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<std::ffi::OsString> = args
            .into_iter()
            .map(|a| a.as_ref().to_os_string())
            .collect();
        let output = Command::new(&self.binary)
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .map_err(SandboxError::PodmanMissing)?;
        if !output.status.success() {
            return Err(SandboxError::PodmanFailed {
                command: args
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Runs podman with `args`, inheriting the terminal (used for attach and interactive runs).
    pub fn run_interactive<I, S>(&self, args: I) -> Result<std::process::ExitStatus>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new(&self.binary)
            .args(args)
            .status()
            .map_err(SandboxError::PodmanMissing)
    }

    /// A pre-configured command for callers that need to own the child's stdio.
    pub fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(&self.binary);
        command.args(args);
        command
    }

    pub fn container_exists(&self, name: &str) -> Result<bool> {
        let output = Command::new(&self.binary)
            .args(["container", "exists", name])
            .output()
            .map_err(SandboxError::PodmanMissing)?;
        Ok(output.status.success())
    }

    pub fn container_is_running(&self, name: &str) -> Result<bool> {
        if !self.container_exists(name)? {
            return Ok(false);
        }
        let status = self.output(["inspect", "--format", "{{.State.Status}}", name])?;
        Ok(status.trim() == "running")
    }

    pub fn remove_container(&self, name: &str) -> Result<()> {
        if self.container_exists(name)? {
            self.output(["rm", "--force", name])?;
        }
        Ok(())
    }
}
