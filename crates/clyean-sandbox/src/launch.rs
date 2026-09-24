// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Composition of the container of one agent from the project, the sandbox
//! configuration, the Herdr context, and the agent's role in a unit of work.

use std::path::PathBuf;

use clyean_agents::profile::{
    container_credentials_bundle_path, container_daemon_dirs, container_overlay_path,
    container_profile_root,
};
use clyean_agents::AgentId;
use clyean_plantuml::render::{CommandOutcome, CommandRunner};
use clyean_project::{LaunchId, ProjectConfig, ProjectDirectory, ProjectId, ProjectLayout};

use crate::container::{
    AgentContainerSpec, ContainerPaths, MountSpec, HARNESS_CONTAINER_PATH,
    ORCHESTRATOR_SOCKET_CONTAINER_PATH, REMOTE_DETACH_KEYS,
};
use crate::environment::{HostPathMapper, Topology};
use crate::herdr::{self, HerdrHostContext};
use crate::roots::SandboxLocation;

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
    pub sandbox: SandboxLocation,
    pub host_paths: HostPathMapper,
    /// The host's Git settings that must hold inside containers too, such as
    /// `core.autocrlf`, so that files do not look modified on the other side.
    pub git_settings: Vec<(String, String)>,
}

/// How an agent participates in work, which decides its interactivity and identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchRole {
    /// The User Assistant of one `clyean` invocation, which reaches that invocation only
    /// through the bridge mounted from `bridge_binary`, a path on the Podman host.
    UserAssistant {
        launch_id: LaunchId,
        bridge_binary: String,
    },
    /// A sub-agent driven over RPC for one unit of work.  It receives the host variables
    /// that `secrets` names (exact names or `*` glob patterns), which are its providers'
    /// variables, besides those the project assigns to it.
    SubAgent {
        work_id: String,
        secrets: Vec<String>,
    },
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
            LaunchRole::SubAgent { work_id, .. } => {
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

    /// The mounts every agent gets: the workspace read-write and extra host paths
    /// read-only under `/mnt`.
    pub fn shared_mounts(&self, paths: &ContainerPaths) -> Vec<MountSpec> {
        let mut mounts = vec![MountSpec::read_write(
            self.host_paths.map(self.directory.workspace()),
            paths.workspace_dir(),
        )];
        for host_path in &self.config.sandbox.mounts {
            let name = host_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "mount".to_string());
            mounts.push(MountSpec::read_only(
                self.host_paths.map(host_path),
                format!("/mnt/{name}"),
            ));
        }
        mounts
    }

    /// Empty file systems over every agent's profile and over `~/.omp/agent`, which no
    /// profile reads, with `agent`'s own profile mounted back from the root file system, so
    /// each container exposes one login store.  Maintenance containers get no profile.
    fn profile_isolation(
        &self,
        agent: AgentId,
        role: &LaunchRole,
    ) -> (Vec<MountSpec>, Vec<String>) {
        let user = self.user.name();
        let masks = vec![
            format!("/home/{user}/.omp/profiles"),
            format!("/home/{user}/.omp/agent"),
        ];
        if matches!(role, LaunchRole::Maintenance) {
            return (Vec::new(), masks);
        }
        let profile = container_profile_root(user, agent);
        let own_profile = MountSpec::read_write(format!("{}{profile}", self.sandbox.root), profile);
        (vec![own_profile], masks)
    }

    /// The names and patterns of the host variables `agent` receives in `role`: the User
    /// Assistant gets the built-in provider patterns and the project's entries written as
    /// names; a sub-agent gets its providers' variables and the project's entries that name
    /// it; maintenance containers get none.
    pub fn secret_patterns<'a>(&'a self, agent: AgentId, role: &'a LaunchRole) -> Vec<&'a str> {
        let is_user_assistant = matches!(role, LaunchRole::UserAssistant { .. });
        let mut patterns: Vec<&str> = match role {
            LaunchRole::UserAssistant { .. } => BUILTIN_PASSTHROUGH_PATTERNS.to_vec(),
            LaunchRole::SubAgent { secrets, .. } => secrets.iter().map(String::as_str).collect(),
            LaunchRole::Maintenance => return Vec::new(),
        };
        patterns.extend(
            self.config
                .sandbox
                .passthrough_env
                .iter()
                .filter(|entry| entry.reaches(agent.id(), is_user_assistant))
                .map(|entry| entry.pattern()),
        );
        patterns
    }

    fn shared_environment(&self, agent: AgentId, paths: &ContainerPaths) -> Vec<(String, String)> {
        let mut env = vec![
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
            ("HOME".to_string(), self.user.home()),
            ("OMP_PROFILE".to_string(), agent.id().to_string()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
        ];
        env.extend(self.git_environment(paths));
        // Herdr, on this host, can open the root filesystem's files only when it lives on
        // this host's kernel.
        if self.sandbox.topology == Topology::SharedKernel {
            env.push((
                "CLYEAN_HOST_CONTAINER_ROOT".to_string(),
                self.sandbox.root.clone(),
            ));
        }
        env.extend(
            agent
                .git_identity()
                .environment()
                .into_iter()
                .map(|(key, value)| (key.to_string(), value)),
        );
        env
    }

    /// Git configuration for every container, through Git's `GIT_CONFIG_*` variables: the
    /// host's line-ending settings, and the workspace and project as safe directories,
    /// because files crossing a shared filesystem can appear owned by another user.
    fn git_environment(&self, paths: &ContainerPaths) -> Vec<(String, String)> {
        let mut settings = self.git_settings.clone();
        for directory in [paths.workspace_dir(), paths.project_dir()] {
            let setting = ("safe.directory".to_string(), directory.to_string());
            if !settings.contains(&setting) {
                settings.push(setting);
            }
        }
        let mut env = vec![("GIT_CONFIG_COUNT".to_string(), settings.len().to_string())];
        for (index, (key, value)) in settings.into_iter().enumerate() {
            env.push((format!("GIT_CONFIG_KEY_{index}"), key));
            env.push((format!("GIT_CONFIG_VALUE_{index}"), value));
        }
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
        let mut mounts = self.shared_mounts(&paths);
        let (profile_mounts, tmpfs_mounts) = self.profile_isolation(agent, &role);
        mounts.extend(profile_mounts);
        let mut environment =
            select_host_variables(std::env::vars(), &self.secret_patterns(agent, &role));
        environment.extend(self.shared_environment(agent, &paths));
        let mut command = vec![HARNESS_CONTAINER_PATH.to_string()];
        command.extend(harness_args);
        let mut spec = AgentContainerSpec {
            name: self.container_name(agent, &role),
            agent,
            rootfs: self.sandbox.root.clone(),
            workdir: paths.project_dir().to_string(),
            labels: vec![self.sandbox.label()],
            mounts,
            tmpfs_mounts,
            environment,
            tty: false,
            remove_on_exit: true,
            extra_run_args: self.config.sandbox.podman_run_args.clone(),
            command,
            detach_keys: match self.sandbox.topology {
                Topology::SharedKernel => "",
                Topology::VirtualMachine => REMOTE_DETACH_KEYS,
            },
        };
        match role {
            LaunchRole::UserAssistant {
                launch_id,
                bridge_binary,
            } => self.add_user_assistant_parts(&mut spec, &launch_id, bridge_binary),
            LaunchRole::SubAgent { work_id, .. } => {
                spec.environment
                    .push(("CLYEAN_WORK_ID".to_string(), work_id));
                spec.environment.push((
                    "CLYEAN_CREDENTIALS_BUNDLE".to_string(),
                    container_credentials_bundle_path(self.user.name(), agent),
                ));
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
        bridge_binary: String,
    ) {
        spec.tty = true;
        spec.labels.extend([
            (PROJECT_LABEL.to_string(), self.project_id.to_string()),
            (LAUNCH_LABEL.to_string(), launch_id.to_string()),
            (ROLE_LABEL.to_string(), USER_ASSISTANT_ROLE.to_string()),
        ]);
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
            spec.mounts.extend(herdr.executable_mount(self.host_paths));
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
    /// container: the orchestrator channel always, the Herdr channel inside Herdr.  The
    /// session has no terminal, so no detach sequence applies to its binary stream; the
    /// local client is still told to turn detaching off, which the remote one rejects.
    pub fn bridge_exec_args(&self, container_name: &str) -> Vec<String> {
        let mut args = vec!["exec".to_string(), "--interactive".to_string()];
        if self.sandbox.topology == Topology::SharedKernel {
            args.push("--detach-keys=".to_string());
        }
        args.extend([
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
        ]);
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

/// Host variables that reach the User Assistant's container so provider credentials
/// configured on the host work inside the sandbox: any `*_API_KEY`, plus the cloud and
/// endpoint variables the harness's providers read.  The harness's auth broker variables
/// are deliberately absent, because a broker hands every client every credential it holds.
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
];

/// The host variables whose names match one of `patterns` (exact names, or `*` glob
/// patterns with a leading or trailing `*`), sorted by name.
pub fn select_host_variables(
    host: impl IntoIterator<Item = (String, String)>,
    patterns: &[&str],
) -> Vec<(String, String)> {
    let mut selected: Vec<(String, String)> = host
        .into_iter()
        .filter(|(name, _)| patterns.iter().any(|pattern| glob_matches(pattern, name)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roots::{SandboxRoots, SANDBOX_LABEL};
    use clyean_project::{ProjectType, SandboxConfig, SandboxId};
    use std::path::Path;

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
            sandbox: SandboxRoots::beside_graph_root("/home/skyei/.local/share/containers/storage")
                .location(
                    SandboxId::try_from("3f9c2a7d1e4b8c05".to_string()).unwrap(),
                    Topology::SharedKernel,
                ),
            host_paths: HostPathMapper::Identity,
            git_settings: vec![("core.autocrlf".into(), "input".into())],
        }
    }

    const ROOT: &str = "/home/skyei/.local/share/clyean/roots/3f9c2a7d1e4b8c05";

    fn user_assistant_role(launch: &str) -> LaunchRole {
        LaunchRole::UserAssistant {
            launch_id: LaunchId::generate(),
            bridge_binary: launch.to_string(),
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
                (SANDBOX_LABEL.to_string(), "3f9c2a7d1e4b8c05".to_string()),
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
            "GIT_CONFIG_COUNT=3",
            "GIT_CONFIG_KEY_0=core.autocrlf",
            "GIT_CONFIG_VALUE_0=input",
            "GIT_CONFIG_KEY_1=safe.directory",
            "GIT_CONFIG_VALUE_1=/home/skyei/workspace/workspace",
            "GIT_CONFIG_KEY_2=safe.directory",
            "GIT_CONFIG_VALUE_2=/home/skyei/workspace/workspace/proj",
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
    fn a_podman_machine_gets_a_detach_sequence_its_remote_client_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path());
        context.sandbox.topology = Topology::VirtualMachine;
        let spec = context.agent_container_spec(
            AgentId::UserAssistant,
            user_assistant_role("/b"),
            Vec::new(),
        );
        assert!(spec
            .run_args()
            .contains(&format!("--detach-keys={REMOTE_DETACH_KEYS}")));
        assert!(!context
            .bridge_exec_args("c")
            .iter()
            .any(|a| a.starts_with("--detach-keys")));
        assert!(!spec
            .environment
            .iter()
            .any(|(name, _)| name == "CLYEAN_HOST_CONTAINER_ROOT"));
    }

    #[test]
    fn host_variables_are_selected_by_exact_names_and_globs() {
        let host = vec![
            ("ANTHROPIC_API_KEY".to_string(), "k1".to_string()),
            ("OPENAI_BASE_URL".to_string(), "https://proxy".to_string()),
            ("HOME".to_string(), "/home/x".to_string()),
            ("MY_PRIVATE_TOKEN".to_string(), "t".to_string()),
            ("CUSTOM_THING".to_string(), "c".to_string()),
        ];
        let selected = select_host_variables(host, &["*_API_KEY", "CUSTOM_THING", "MY_*"]);
        let names: Vec<&str> = selected.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["ANTHROPIC_API_KEY", "CUSTOM_THING", "MY_PRIVATE_TOKEN"]
        );
    }

    #[test]
    fn host_secrets_reach_only_the_agents_that_need_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path());
        context.config.sandbox.passthrough_env = serde_json::from_str(
            r#"["CORP_PROXY_TOKEN", {"name": "GH_TOKEN", "agents": ["software-engineering-director"]}]"#,
        )
        .unwrap();
        let user_assistant = user_assistant_role("/b");
        let patterns = context.secret_patterns(AgentId::UserAssistant, &user_assistant);
        assert!(patterns.contains(&"*_API_KEY"));
        assert!(patterns.contains(&"CORP_PROXY_TOKEN"));
        assert!(!patterns.contains(&"GH_TOKEN"));
        assert!(!patterns.iter().any(|p| p.starts_with("OMP_AUTH_BROKER")));

        let director = LaunchRole::SubAgent {
            work_id: "w".into(),
            secrets: vec!["OPENAI_API_KEY".into()],
        };
        assert_eq!(
            context.secret_patterns(AgentId::SoftwareEngineeringDirector, &director),
            ["OPENAI_API_KEY", "GH_TOKEN"]
        );
        let programmer = LaunchRole::SubAgent {
            work_id: "w".into(),
            secrets: Vec::new(),
        };
        assert!(context
            .secret_patterns(AgentId::Programmer, &programmer)
            .is_empty());
        assert!(context
            .secret_patterns(AgentId::SoftwareArchitect, &LaunchRole::Maintenance)
            .is_empty());
    }

    #[test]
    fn every_container_exposes_at_most_its_own_profile() {
        let dir = tempfile::tempdir().unwrap();
        let context = context(dir.path());
        let masks = [
            "/home/skyei/.omp/profiles".to_string(),
            "/home/skyei/.omp/agent".to_string(),
        ];
        let cases = [
            (AgentId::UserAssistant, user_assistant_role("/b")),
            (
                AgentId::Programmer,
                LaunchRole::SubAgent {
                    work_id: "w".into(),
                    secrets: Vec::new(),
                },
            ),
        ];
        for (agent, role) in cases {
            let spec = context.agent_container_spec(agent, role, Vec::new());
            for mask in &masks {
                assert!(spec.tmpfs_mounts.contains(mask), "{agent:?} masks {mask}");
            }
            let profiles: Vec<&MountSpec> = spec
                .mounts
                .iter()
                .filter(|m| m.target.starts_with("/home/skyei/.omp"))
                .collect();
            assert_eq!(
                profiles,
                [&MountSpec::read_write(
                    format!("{ROOT}/home/skyei/.omp/profiles/{}", agent.id()),
                    format!("/home/skyei/.omp/profiles/{}", agent.id()),
                )]
            );
        }
        let maintenance = context.agent_container_spec(
            AgentId::SoftwareArchitect,
            LaunchRole::Maintenance,
            Vec::new(),
        );
        for mask in &masks {
            assert!(maintenance.tmpfs_mounts.contains(mask));
        }
        assert!(!maintenance
            .mounts
            .iter()
            .any(|m| m.target.starts_with("/home/skyei/.omp")));
    }

    #[test]
    fn sub_agent_container_is_headless_and_scoped_to_its_work() {
        let dir = tempfile::tempdir().unwrap();
        let context = context(dir.path());
        let args = context.sub_agent_harness_args(AgentId::Programmer, Some("/sessions/x.jsonl"));
        let role = LaunchRole::SubAgent {
            work_id: "0192a-work".into(),
            secrets: Vec::new(),
        };
        let spec = context.agent_container_spec(AgentId::Programmer, role, args);
        assert!(!spec.tty);
        assert!(spec.remove_on_exit);
        assert!(spec.name.ends_with("-programmer-0192awor"));
        assert_eq!(
            spec.labels,
            [(SANDBOX_LABEL.to_string(), "3f9c2a7d1e4b8c05".to_string())]
        );
        assert!(spec
            .tmpfs_mounts
            .contains(&"/home/skyei/.omp/profiles/programmer/run".to_string()));
        let run = spec.run_args();
        assert!(run.iter().any(|a| a == "CLYEAN_WORK_ID=0192a-work"));
        assert!(run.iter().any(|a| a
            == "CLYEAN_CREDENTIALS_BUNDLE=/home/skyei/.omp/profiles/programmer/agent/clyean-credentials.json"));
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
