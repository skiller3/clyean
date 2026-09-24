// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Composition of the container of one agent from the project, the sandbox
//! configuration, the Herdr context, and the agent's role in a unit of work.

use std::path::{Path, PathBuf};

use clyean_agents::profile::{container_daemon_dirs, container_overlay_path};
use clyean_agents::AgentId;
use clyean_plantuml::render::{CommandOutcome, CommandRunner};
use clyean_project::{LaunchId, ProjectConfig, ProjectDirectory, ProjectId, ProjectLayout};

use crate::container::{
    AgentContainerSpec, ContainerPaths, MountSpec, HARNESS_CONTAINER_PATH,
    ORCHESTRATOR_SOCKET_CONTAINER_PATH,
};
use crate::herdr::{self, HerdrHostContext};

/// Labels that identify the User Assistant containers of every project.
pub const PROJECT_LABEL: &str = "clyean.project";
pub const LAUNCH_LABEL: &str = "clyean.launch";
pub const ROLE_LABEL: &str = "clyean.role";
pub const USER_ASSISTANT_ROLE: &str = "user-assistant";

/// In-container directories of the bridge's sockets, private to each User Assistant.
const BRIDGE_SOCKET_DIRS: [&str; 2] = ["/run/clyean", "/run/herdr"];
use crate::podman::Podman;
use crate::user::ContainerUser;

/// Everything needed to describe any agent container of one project.
#[derive(Debug, Clone)]
pub struct LaunchContext {
    pub podman: Podman,
    pub directory: ProjectDirectory,
    pub layout: ProjectLayout,
    pub project_id: ProjectId,
    pub config: ProjectConfig,
    pub user: ContainerUser,
    pub clyean_version: String,
    pub herdr: Option<HerdrHostContext>,
}

/// How an agent participates in work, which decides its interactivity and identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchRole {
    /// The User Assistant of one `clyean` invocation, which reaches that invocation only
    /// through the bridge mounted from `bridge_binary` on the host.
    UserAssistant {
        launch_id: LaunchId,
        bridge_binary: PathBuf,
    },
    /// A sub-agent driven over RPC for one unit of work.
    SubAgent { work_id: String },
    /// A one-off command run inside the sandbox by Clyean itself.
    Maintenance,
}

impl LaunchContext {
    pub fn container_paths(&self) -> ContainerPaths {
        ContainerPaths::new(
            &self.user,
            self.directory.workspace(),
            &self.directory.workspace_name(),
            self.directory.project_relative_to_workspace(),
        )
    }

    pub fn container_name(&self, agent: AgentId, role: &LaunchRole) -> String {
        match role {
            LaunchRole::UserAssistant { launch_id, .. } => {
                format!("clyean-{}-{}-{launch_id}", self.project_id, agent.id())
            }
            LaunchRole::SubAgent { work_id } => {
                let short: String = work_id
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .take(8)
                    .collect();
                format!("clyean-{}-{}-{short}", self.project_id, agent.id())
            }
            LaunchRole::Maintenance => format!(
                "clyean-{}-maintenance-{}",
                self.project_id,
                std::process::id()
            ),
        }
    }

    /// The mounts every agent gets: the workspace read-write, extra host paths read-only
    /// under `/mnt`, and the mask over `.clyean/container-root`.
    fn shared_mounts(&self, paths: &ContainerPaths) -> (Vec<MountSpec>, Vec<String>) {
        let mut mounts = vec![MountSpec::read_write(
            self.directory.workspace(),
            paths.workspace_dir(),
        )];
        for host_path in &self.config.sandbox.mounts {
            let name = host_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "mount".to_string());
            mounts.push(MountSpec::read_only(
                host_path.clone(),
                format!("/mnt/{name}"),
            ));
        }
        (mounts, vec![paths.container_root_mask()])
    }

    fn shared_environment(&self, agent: AgentId, paths: &ContainerPaths) -> Vec<(String, String)> {
        let mut env =
            passthrough_environment(std::env::vars(), &self.config.sandbox.passthrough_env);
        env.extend(vec![
            ("CLYEAN_AGENT".to_string(), agent.id().to_string()),
            ("CLYEAN_VERSION".to_string(), self.clyean_version.clone()),
            (
                "CLYEAN_PROJECT_DIR".to_string(),
                paths.project_dir().to_string(),
            ),
            (
                "CLYEAN_WORKSPACE_DIR".to_string(),
                paths.workspace_dir().to_string(),
            ),
            (
                "CLYEAN_HOST_WORKSPACE_DIR".to_string(),
                self.directory.workspace().to_string_lossy().into_owned(),
            ),
            (
                "CLYEAN_HOST_CONTAINER_ROOT".to_string(),
                self.layout
                    .container_root_dir()
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("HOME".to_string(), self.user.home()),
            ("OMP_PROFILE".to_string(), agent.id().to_string()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
        ]);
        env.extend(
            agent
                .git_identity()
                .environment()
                .into_iter()
                .map(|(key, value)| (key.to_string(), value)),
        );
        env
    }

    /// The `podman run` description of `agent` in `role` running the harness with
    /// `harness_args`.  A User Assistant's harness starts only once its bridge is serving.
    pub fn agent_container_spec(
        &self,
        agent: AgentId,
        role: LaunchRole,
        harness_args: Vec<String>,
    ) -> AgentContainerSpec {
        let paths = self.container_paths();
        let (mounts, tmpfs_mounts) = self.shared_mounts(&paths);
        let mut command = vec![HARNESS_CONTAINER_PATH.to_string()];
        command.extend(harness_args);
        let mut spec = AgentContainerSpec {
            name: self.container_name(agent, &role),
            agent,
            rootfs: self.layout.container_root_dir(),
            workdir: paths.project_dir().to_string(),
            labels: Vec::new(),
            mounts,
            tmpfs_mounts,
            environment: self.shared_environment(agent, &paths),
            tty: false,
            remove_on_exit: true,
            extra_run_args: self.config.sandbox.podman_run_args.clone(),
            command,
        };
        match role {
            LaunchRole::UserAssistant {
                launch_id,
                bridge_binary,
            } => self.add_user_assistant_parts(&mut spec, &launch_id, bridge_binary),
            LaunchRole::SubAgent { work_id } => {
                spec.environment
                    .push(("CLYEAN_WORK_ID".to_string(), work_id));
                spec.tmpfs_mounts
                    .extend(container_daemon_dirs(self.user.name(), agent));
            }
            LaunchRole::Maintenance => {}
        }
        spec
    }

    fn add_user_assistant_parts(
        &self,
        spec: &mut AgentContainerSpec,
        launch_id: &LaunchId,
        bridge_binary: PathBuf,
    ) {
        spec.tty = true;
        spec.labels = vec![
            (PROJECT_LABEL.to_string(), self.project_id.to_string()),
            (LAUNCH_LABEL.to_string(), launch_id.to_string()),
            (ROLE_LABEL.to_string(), USER_ASSISTANT_ROLE.to_string()),
        ];
        for key in ["TERM", "COLORTERM", "TERM_PROGRAM"] {
            if let Ok(value) = std::env::var(key) {
                spec.environment.push((key.to_string(), value));
            }
        }
        spec.environment.push((
            "CLYEAN_ORCHESTRATOR_SOCKET".to_string(),
            ORCHESTRATOR_SOCKET_CONTAINER_PATH.to_string(),
        ));
        spec.environment
            .push(("CLYEAN_ORCHESTRATOR_LEASE".to_string(), "1".to_string()));
        if let Some(herdr) = &self.herdr {
            spec.environment.extend(herdr.container_environment());
            spec.mounts.extend(herdr.executable_mount());
        }
        spec.mounts.push(MountSpec::read_only(
            bridge_binary,
            clyean_bridge::CONTAINER_PATH,
        ));
        spec.tmpfs_mounts
            .extend(BRIDGE_SOCKET_DIRS.iter().map(|dir| dir.to_string()));
        spec.tmpfs_mounts
            .extend(container_daemon_dirs(self.user.name(), spec.agent));
        let mut gated = vec![
            clyean_bridge::CONTAINER_PATH.to_string(),
            "await".to_string(),
            "--ready-file".to_string(),
            clyean_bridge::READY_FILE.to_string(),
            "--".to_string(),
        ];
        gated.append(&mut spec.command);
        spec.command = gated;
    }

    /// Arguments after `podman` that open the bridge session of a running User Assistant
    /// container: the orchestrator channel always, the Herdr channel inside Herdr.
    pub fn bridge_exec_args(&self, container_name: &str) -> Vec<String> {
        let mut args = vec![
            "exec".to_string(),
            "--interactive".to_string(),
            "--detach-keys=".to_string(),
            container_name.to_string(),
            clyean_bridge::CONTAINER_PATH.to_string(),
            "bridge".to_string(),
            "--ready-file".to_string(),
            clyean_bridge::READY_FILE.to_string(),
            "--channel".to_string(),
            format!(
                "{}={ORCHESTRATOR_SOCKET_CONTAINER_PATH}",
                clyean_bridge::ORCHESTRATOR_CHANNEL
            ),
        ];
        if self.herdr.is_some() {
            args.push("--channel".to_string());
            args.push(format!(
                "{}={}",
                clyean_bridge::HERDR_CHANNEL,
                herdr::CONTAINER_SOCKET_PATH
            ));
        }
        args
    }

    /// Harness arguments common to every agent: the project directory and the tracked
    /// settings overlay of the agent's profile.
    pub fn base_harness_args(&self, agent: AgentId) -> Vec<String> {
        let paths = self.container_paths();
        vec![
            "--cwd".to_string(),
            paths.project_dir().to_string(),
            "--config".to_string(),
            container_overlay_path(self.user.name(), agent),
        ]
    }

    /// Harness arguments of a sub-agent session driven over RPC.
    pub fn sub_agent_harness_args(
        &self,
        agent: AgentId,
        resume_session: Option<&str>,
    ) -> Vec<String> {
        let mut args = self.base_harness_args(agent);
        args.extend(["--mode", "rpc", "--approval-mode", "yolo", "--no-title"].map(String::from));
        if let Some(session) = resume_session {
            args.push("--resume".to_string());
            args.push(session.to_string());
        }
        args
    }

    /// A runner that executes commands inside the sandbox with the workspace mounted,
    /// used for diagram rendering and other maintenance.
    pub fn maintenance_runner(&self) -> SandboxRunner {
        SandboxRunner {
            context: self.clone(),
        }
    }
}

/// Runs one-off commands in a maintenance container of the project.
#[derive(Debug, Clone)]
pub struct SandboxRunner {
    context: LaunchContext,
}

impl CommandRunner for SandboxRunner {
    fn run(&self, argv: &[String]) -> std::io::Result<CommandOutcome> {
        let mut spec = self.context.agent_container_spec(
            AgentId::SoftwareArchitect,
            LaunchRole::Maintenance,
            Vec::new(),
        );
        spec.command = argv.to_vec();
        let output = self.context.podman.command(spec.run_args()).output()?;
        Ok(CommandOutcome {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Host variables that reach every agent container so provider credentials configured on
/// the host work inside the sandbox: any `*_API_KEY`, plus the cloud and endpoint
/// variables the harness's providers read.
pub const BUILTIN_PASSTHROUGH_PATTERNS: &[&str] = &[
    "*_API_KEY",
    "*_API_TOKEN",
    "*_BASE_URL",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_PROFILE",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AZURE_OPENAI_ENDPOINT",
    "AZURE_OPENAI_API_VERSION",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_LOCATION",
    "OMP_AUTH_BROKER_URL",
    "OMP_AUTH_BROKER_TOKEN",
];

/// Selects the host variables to pass through: those matching a built-in pattern or one of
/// the project's extra names or `*` glob patterns.
pub fn passthrough_environment(
    host: impl IntoIterator<Item = (String, String)>,
    extra_patterns: &[String],
) -> Vec<(String, String)> {
    let mut selected: Vec<(String, String)> = host
        .into_iter()
        .filter(|(name, _)| {
            BUILTIN_PASSTHROUGH_PATTERNS
                .iter()
                .any(|pattern| glob_matches(pattern, name))
                || extra_patterns
                    .iter()
                    .any(|pattern| glob_matches(pattern, name))
        })
        .collect();
    selected.sort();
    selected
}

fn glob_matches(pattern: &str, name: &str) -> bool {
    match (pattern.strip_prefix('*'), pattern.strip_suffix('*')) {
        (Some(suffix), _) if !suffix.contains('*') => name.ends_with(suffix),
        (_, Some(prefix)) if !prefix.contains('*') => name.starts_with(prefix),
        _ => pattern == name,
    }
}

/// Where Clyean caches downloads (the harness binary and the PlantUML jar).
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(dir).join("clyean");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("clyean");
    }
    std::env::temp_dir().join("clyean-cache")
}

#[allow(dead_code)]
fn is_dir(path: &Path) -> bool {
    path.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clyean_project::{ProjectType, SandboxConfig};

    fn context(dir: &Path) -> LaunchContext {
        let project = dir.join("workspace").join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let directory =
            ProjectDirectory::resolve(Some(&project), Some(&dir.join("workspace"))).unwrap();
        let layout = ProjectLayout::new(directory.project());
        let mut config = ProjectConfig::new(
            "0.1.0",
            ProjectType::SoftwareEngineeringProject,
            directory.workspace(),
            false,
            SandboxConfig::with_image("docker.io/library/ubuntu:latest"),
        );
        config.sandbox.mounts = vec![PathBuf::from("/host/reference-data")];
        config.sandbox.podman_run_args = vec!["--memory".into(), "8g".into()];
        LaunchContext {
            podman: Podman::default(),
            project_id: ProjectId::of(directory.project()),
            directory,
            layout,
            config,
            user: ContainerUser::from_host_user_name("skyei"),
            clyean_version: "0.1.0".into(),
            herdr: Some(HerdrHostContext {
                pane_id: "w1:p1".into(),
                tab_id: None,
                workspace_id: None,
                socket_path: PathBuf::from("/tmp/herdr.sock"),
                bin_path: None,
            }),
        }
    }

    fn user_assistant_role(launch: &str) -> LaunchRole {
        LaunchRole::UserAssistant {
            launch_id: LaunchId::generate(),
            bridge_binary: PathBuf::from(launch),
        }
    }

    #[test]
    fn user_assistant_container_is_private_to_its_launch_and_gated_on_its_bridge() {
        let dir = tempfile::tempdir().unwrap();
        let context = context(dir.path());
        let args = context.base_harness_args(AgentId::UserAssistant);
        let role = user_assistant_role("/home/skyei/.cache/clyean/bridge/clyean-bridge");
        let LaunchRole::UserAssistant { launch_id, .. } = &role else {
            unreachable!()
        };
        let launch = launch_id.to_string();
        let spec = context.agent_container_spec(AgentId::UserAssistant, role, args);
        assert!(spec.tty);
        assert!(spec.remove_on_exit);
        assert!(spec.name.ends_with(&format!("-user-assistant-{launch}")));
        assert_eq!(
            spec.labels,
            vec![
                (PROJECT_LABEL.to_string(), context.project_id.to_string()),
                (LAUNCH_LABEL.to_string(), launch.clone()),
                (ROLE_LABEL.to_string(), USER_ASSISTANT_ROLE.to_string()),
            ]
        );
        for private in [
            "/run/clyean",
            "/run/herdr",
            "/home/skyei/.omp/run",
            "/home/skyei/.omp/profiles/user-assistant/run",
        ] {
            assert!(
                spec.tmpfs_mounts.contains(&private.to_string()),
                "{private}"
            );
        }
        let run = spec.run_args();
        for expected in [
            "--detach-keys=",
            "CLYEAN_AGENT=user-assistant",
            "OMP_PROFILE=user-assistant",
            "HERDR_SOCKET_PATH=/run/herdr/herdr.sock",
            "CLYEAN_ORCHESTRATOR_SOCKET=/run/clyean/orchestrator.sock",
            "CLYEAN_ORCHESTRATOR_LEASE=1",
            "GIT_AUTHOR_NAME=Clyean User Assistant",
            "type=bind,src=/home/skyei/.cache/clyean/bridge/clyean-bridge,dst=/usr/local/libexec/clyean/clyean-bridge,ro=true",
            "--memory",
        ] {
            assert!(run.contains(&expected.to_string()), "missing {expected}");
        }
        assert!(run
            .iter()
            .any(|a| a.contains("dst=/mnt/reference-data,ro=true")));
        assert!(
            !run.iter()
                .any(|a| a.contains("herdr.sock,") || a.contains(".sock,dst")),
            "no socket is bind-mounted"
        );
        let rootfs = run.iter().position(|a| a == "--rootfs").unwrap();
        let command = &run[rootfs + 2..];
        assert_eq!(
            &command[..5],
            [
                "/usr/local/libexec/clyean/clyean-bridge",
                "await",
                "--ready-file",
                "/run/clyean/bridge.ready",
                "--"
            ]
        );
        assert_eq!(command[5], "/usr/local/bin/clyean");
        assert_eq!(command[6], "--cwd");
        assert!(command[9].ends_with("/.omp/profiles/user-assistant/agent/clyean-overlay.json"));
    }

    #[test]
    fn concurrent_launches_get_distinct_containers() {
        let dir = tempfile::tempdir().unwrap();
        let context = context(dir.path());
        let first = context.container_name(AgentId::UserAssistant, &user_assistant_role("/b"));
        let second = context.container_name(AgentId::UserAssistant, &user_assistant_role("/b"));
        assert_ne!(first, second);
    }

    #[test]
    fn the_bridge_session_serves_herdr_only_inside_herdr() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path());
        let args = context.bridge_exec_args("clyean-p-user-assistant-1234abcd");
        assert_eq!(
            &args[..6],
            [
                "exec",
                "--interactive",
                "--detach-keys=",
                "clyean-p-user-assistant-1234abcd",
                "/usr/local/libexec/clyean/clyean-bridge",
                "bridge"
            ]
        );
        assert!(args.contains(&"orchestrator=/run/clyean/orchestrator.sock".to_string()));
        assert!(args.contains(&"herdr=/run/herdr/herdr.sock".to_string()));
        context.herdr = None;
        let args = context.bridge_exec_args("c");
        assert!(!args.iter().any(|a| a.starts_with("herdr=")));
    }

    #[test]
    fn provider_credentials_and_configured_names_pass_through() {
        let host = vec![
            ("ANTHROPIC_API_KEY".to_string(), "k1".to_string()),
            ("OPENAI_BASE_URL".to_string(), "https://proxy".to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), "s".to_string()),
            ("HOME".to_string(), "/home/x".to_string()),
            ("MY_PRIVATE_TOKEN".to_string(), "t".to_string()),
            ("CUSTOM_THING".to_string(), "c".to_string()),
        ];
        let selected =
            passthrough_environment(host, &["CUSTOM_THING".to_string(), "MY_*".to_string()]);
        let names: Vec<&str> = selected.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "ANTHROPIC_API_KEY",
                "AWS_SECRET_ACCESS_KEY",
                "CUSTOM_THING",
                "MY_PRIVATE_TOKEN",
                "OPENAI_BASE_URL"
            ]
        );
    }

    #[test]
    fn sub_agent_container_is_headless_and_scoped_to_its_work() {
        let dir = tempfile::tempdir().unwrap();
        let context = context(dir.path());
        let args = context.sub_agent_harness_args(AgentId::Programmer, Some("/sessions/x.jsonl"));
        let role = LaunchRole::SubAgent {
            work_id: "0192a-work".into(),
        };
        let spec = context.agent_container_spec(AgentId::Programmer, role, args);
        assert!(!spec.tty);
        assert!(spec.remove_on_exit);
        assert!(spec.name.ends_with("-programmer-0192awor"));
        assert!(spec.labels.is_empty());
        assert!(spec
            .tmpfs_mounts
            .contains(&"/home/skyei/.omp/profiles/programmer/run".to_string()));
        let run = spec.run_args();
        assert!(run.iter().any(|a| a == "CLYEAN_WORK_ID=0192a-work"));
        assert!(!run.iter().any(|a| a.starts_with("HERDR_")));
        assert!(!run
            .iter()
            .any(|a| a.starts_with("CLYEAN_ORCHESTRATOR_SOCKET")));
        assert!(run.windows(2).any(|w| w[0] == "--mode" && w[1] == "rpc"));
        assert!(run
            .windows(2)
            .any(|w| w[0] == "--resume" && w[1] == "/sessions/x.jsonl"));
        assert!(run
            .windows(2)
            .any(|w| w[0] == "--approval-mode" && w[1] == "yolo"));
    }
}
