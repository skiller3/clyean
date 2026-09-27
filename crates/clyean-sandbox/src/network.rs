// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Each project's Podman network.  The User Assistant's container, every sub-agent
//! container, and every server container of a project join it, so an agent reaches a
//! server by its name, on the server's own port, without anything exposed on the host.
//! Clyean's project networks are isolated from each other, and, like Podman's own
//! networking, none of them reaches the host's loopback.

use clyean_project::{ProjectId, SandboxId};

use crate::launch::PROJECT_LABEL;
use crate::podman::Podman;
use crate::roots::SANDBOX_LABEL;
use crate::{Result, SandboxError};

/// One project's network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectNetwork {
    name: String,
}

impl ProjectNetwork {
    pub fn for_project(project_id: &ProjectId) -> Self {
        Self {
            name: format!("clyean-{project_id}"),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The `podman` arguments that create the network: isolated from Clyean's other
    /// project networks and labeled with its project and sandbox.
    pub fn create_args(&self, project_id: &ProjectId, sandbox_id: &SandboxId) -> Vec<String> {
        vec![
            "network".into(),
            "create".into(),
            "--opt".into(),
            "isolate=true".into(),
            "--label".into(),
            format!("{PROJECT_LABEL}={project_id}"),
            "--label".into(),
            format!("{SANDBOX_LABEL}={sandbox_id}"),
            self.name.clone(),
        ]
    }

    /// Creates the network unless it exists.  A creation that loses a race with another
    /// `clyean` process finds the network there, which counts as success.
    pub fn ensure(
        &self,
        podman: &Podman,
        project_id: &ProjectId,
        sandbox_id: &SandboxId,
    ) -> Result<()> {
        if self.exists(podman)? {
            return Ok(());
        }
        match podman.output(self.create_args(project_id, sandbox_id)) {
            Ok(_) => Ok(()),
            Err(error) if self.exists(podman).unwrap_or(false) => {
                tracing::debug!(target: "clyean::network", %error, "another process created the network first");
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn exists(&self, podman: &Podman) -> Result<bool> {
        let output = podman
            .command(["network", "exists", &self.name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(SandboxError::PodmanMissing)?;
        Ok(output.success())
    }
}

/// Removes the project networks of a sandbox that is being removed.  A network some
/// container still uses stays, and is reported.
pub fn remove_sandbox_networks(podman: &Podman, sandbox_id: &SandboxId) -> Vec<String> {
    let listed = podman.output([
        "network",
        "ls",
        "--filter",
        &format!("label={SANDBOX_LABEL}={sandbox_id}"),
        "--format",
        "{{.Name}}",
    ]);
    let mut problems = Vec::new();
    for name in listed.unwrap_or_default().split_whitespace() {
        if let Err(error) = podman.output(["network", "rm", name]) {
            problems.push(format!("{name}: {error}"));
        }
    }
    problems
}

/// What a launch warns about the project network: a `--network` among the project's
/// `podman run` arguments overrides it, and without aardvark-dns names do not resolve.
pub fn warnings(podman_run_args: &[String], names_resolve: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    let overridden = podman_run_args.iter().any(|argument| {
        argument == "--network"
            || argument == "--net"
            || argument.starts_with("--network=")
            || argument.starts_with("--net=")
    });
    if overridden {
        warnings.push(
            "sandbox.podmanRunArgs sets a network, which replaces the project network: agents cannot reach server containers".to_string(),
        );
    }
    if !names_resolve {
        warnings.push(
            "Podman has no aardvark-dns, so agents reach server containers by the address a launch returns, not by name".to_string(),
        );
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_network_is_named_for_its_project_isolated_and_labeled() {
        let project = ProjectId::of(Path::new("/home/skyei/workspace/app"));
        let sandbox = SandboxId::try_from("3f9c2a7d1e4b8c05".to_string()).unwrap();
        let network = ProjectNetwork::for_project(&project);
        assert_eq!(network.name(), format!("clyean-{project}"));
        let args = network.create_args(&project, &sandbox);
        assert_eq!(&args[..4], ["network", "create", "--opt", "isolate=true"]);
        assert!(args.contains(&format!("clyean.project={project}")));
        assert!(args.contains(&"clyean.sandbox=3f9c2a7d1e4b8c05".to_string()));
        assert_eq!(args.last(), Some(&network.name().to_string()));
    }

    #[test]
    fn a_network_override_and_missing_name_resolution_are_warned_about() {
        assert!(warnings(&["--memory".into(), "8g".into()], true).is_empty());
        for overriding in [
            vec!["--network".to_string(), "host".to_string()],
            vec!["--network=host".to_string()],
            vec!["--net=none".to_string()],
        ] {
            assert_eq!(warnings(&overriding, true).len(), 1, "{overriding:?}");
        }
        assert!(warnings(&[], false)[0].contains("aardvark-dns"));
    }
}
