// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Pure composition of `podman run` invocations for agent containers.  Everything here is
//! deterministic and side-effect free so that the exact sandbox boundary is unit-testable.

use std::path::{Path, PathBuf};

use clyean_agents::AgentId;

use crate::user::ContainerUser;

pub const ORCHESTRATOR_SOCKET_CONTAINER_PATH: &str = "/run/clyean/orchestrator.sock";
pub const HARNESS_CONTAINER_PATH: &str = "/usr/local/bin/clyean";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountSpec {
    pub source: PathBuf,
    pub target: String,
    pub read_only: bool,
}

impl MountSpec {
    pub fn read_only(source: impl Into<PathBuf>, target: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            read_only: true,
        }
    }

    pub fn read_write(source: impl Into<PathBuf>, target: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            read_only: false,
        }
    }

    fn podman_argument(&self) -> String {
        let mut argument = format!(
            "type=bind,src={},dst={}",
            self.source.display(),
            self.target
        );
        if self.read_only {
            argument.push_str(",ro=true");
        }
        argument
    }
}

/// Container-side paths of a project, derived from the workspace mount convention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPaths {
    workspace_dir: String,
    project_dir: String,
    host_workspace_dir: PathBuf,
}

impl ContainerPaths {
    pub fn new(
        user: &ContainerUser,
        host_workspace_dir: &Path,
        workspace_name: &str,
        project_relative: &Path,
    ) -> Self {
        let workspace_dir = format!("{}/{workspace_name}", user.workspace_parent());
        let project_dir = join_container_path(&workspace_dir, project_relative);
        Self {
            workspace_dir,
            project_dir,
            host_workspace_dir: host_workspace_dir.to_path_buf(),
        }
    }

    pub fn workspace_dir(&self) -> &str {
        &self.workspace_dir
    }

    pub fn project_dir(&self) -> &str {
        &self.project_dir
    }

    pub fn architecture_dir(&self) -> String {
        format!("{}/.clyean/architecture", self.project_dir)
    }

    /// The path that must be masked so agents cannot traverse the root filesystem
    /// through the workspace mount.
    pub fn container_root_mask(&self) -> String {
        format!("{}/.clyean/container-root", self.project_dir)
    }

    /// Translates a host path inside the workspace to its container path.
    pub fn translate_host_path(&self, host_path: &Path) -> Option<String> {
        let relative = host_path.strip_prefix(&self.host_workspace_dir).ok()?;
        Some(join_container_path(&self.workspace_dir, relative))
    }
}

fn join_container_path(base: &str, relative: &Path) -> String {
    let mut joined = base.to_string();
    for component in relative.components() {
        if let std::path::Component::Normal(part) = component {
            joined.push('/');
            joined.push_str(&part.to_string_lossy());
        }
    }
    joined
}

/// A fully described agent container, ready to be turned into `podman run` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentContainerSpec {
    pub name: String,
    pub agent: AgentId,
    pub rootfs: PathBuf,
    pub workdir: String,
    pub labels: Vec<(String, String)>,
    pub mounts: Vec<MountSpec>,
    /// Paths that get a private, empty in-memory filesystem in this container.
    pub tmpfs_mounts: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub tty: bool,
    pub remove_on_exit: bool,
    pub extra_run_args: Vec<String>,
    pub command: Vec<String>,
}

impl AgentContainerSpec {
    /// The complete argument vector after `podman`.  Every option precedes the rootfs
    /// path because Podman stops parsing options at the first positional argument.
    /// Detaching is always disabled: no one reattaches to an agent container, and the key
    /// sequence must never be taken out of an agent's input.
    pub fn run_args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "run".into(),
            "--init".into(),
            "--name".into(),
            self.name.clone(),
            "--interactive".into(),
            "--detach-keys=".into(),
        ];
        if self.tty {
            args.push("--tty".into());
        }
        if self.remove_on_exit {
            args.push("--rm".into());
        }
        for (key, value) in &self.labels {
            args.push("--label".into());
            args.push(format!("{key}={value}"));
        }
        args.push("--workdir".into());
        args.push(self.workdir.clone());
        for mount in &self.mounts {
            args.push("--mount".into());
            args.push(mount.podman_argument());
        }
        // Without notmpcopyup, Podman copies whatever the root filesystem holds at the path
        // into memory: for the mask over the in-project root filesystem, all of it.
        for path in &self.tmpfs_mounts {
            args.push("--mount".into());
            args.push(format!("type=tmpfs,dst={path},notmpcopyup"));
        }
        for (key, value) in &self.environment {
            args.push("--env".into());
            args.push(format!("{key}={value}"));
        }
        args.extend(self.extra_run_args.iter().cloned());
        args.push("--rootfs".into());
        args.push(self.rootfs.to_string_lossy().into_owned());
        args.extend(self.command.iter().cloned());
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> ContainerUser {
        ContainerUser::from_host_user_name("skyei")
    }

    #[test]
    fn container_paths_follow_the_workspace_mount_convention() {
        let paths = ContainerPaths::new(
            &user(),
            Path::new("/home/skyei/workspace"),
            "workspace",
            Path::new("clyean"),
        );
        assert_eq!(paths.workspace_dir(), "/home/skyei/workspace/workspace");
        assert_eq!(
            paths.project_dir(),
            "/home/skyei/workspace/workspace/clyean"
        );
        assert_eq!(
            paths.container_root_mask(),
            "/home/skyei/workspace/workspace/clyean/.clyean/container-root"
        );
        assert_eq!(
            paths.translate_host_path(Path::new("/home/skyei/workspace/clyean/src/main.rs")),
            Some("/home/skyei/workspace/workspace/clyean/src/main.rs".to_string())
        );
        assert_eq!(paths.translate_host_path(Path::new("/etc/passwd")), None);
    }

    #[test]
    fn project_equal_to_workspace_has_no_relative_component() {
        let paths = ContainerPaths::new(&user(), Path::new("/p/example"), "example", Path::new(""));
        assert_eq!(paths.project_dir(), "/home/skyei/workspace/example");
    }

    #[test]
    fn run_args_place_every_option_before_the_rootfs_path() {
        let spec = AgentContainerSpec {
            name: "clyean-abc-programmer".into(),
            agent: AgentId::Programmer,
            rootfs: PathBuf::from("/p/.clyean/container-root"),
            workdir: "/home/skyei/workspace/p".into(),
            labels: vec![("clyean.role".into(), "user-assistant".into())],
            mounts: vec![
                MountSpec::read_write("/host/p", "/home/skyei/workspace/p"),
                MountSpec::read_only("/host/data", "/mnt/data"),
            ],
            tmpfs_mounts: vec!["/home/skyei/workspace/p/.clyean/container-root".into()],
            environment: vec![("CLYEAN_AGENT".into(), "programmer".into())],
            tty: false,
            remove_on_exit: true,
            extra_run_args: vec!["--memory".into(), "4g".into()],
            command: vec!["clyean".into(), "--mode".into(), "rpc".into()],
        };
        let args = spec.run_args();
        let rootfs_index = args.iter().position(|a| a == "--rootfs").unwrap();
        assert_eq!(args[rootfs_index + 1], "/p/.clyean/container-root");
        assert_eq!(&args[rootfs_index + 2..], ["clyean", "--mode", "rpc"]);
        let options = &args[..rootfs_index];
        for expected in [
            "--init",
            "--rm",
            "--detach-keys=",
            "clyean.role=user-assistant",
            "type=bind,src=/host/data,dst=/mnt/data,ro=true",
            "type=bind,src=/host/p,dst=/home/skyei/workspace/p",
            "type=tmpfs,dst=/home/skyei/workspace/p/.clyean/container-root,notmpcopyup",
            "CLYEAN_AGENT=programmer",
            "--memory",
        ] {
            assert!(
                options.contains(&expected.to_string()),
                "missing {expected}"
            );
        }
        assert!(!args.contains(&"--tty".to_string()));
    }
}
