// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Server containers: commands agents asked to keep running, each in a container of its
//! own that shares the project's root filesystem and workspace, joins the project network
//! under the server's name, and publishes only the relays the user approved.  They get no
//! login store and no host variables.  Labels on each container record what its launch
//! asked for, so every `clyean` process of the project can list and stop it.

use std::collections::HashMap;
use std::net::IpAddr;

use clyean_project::layout::{CLYEAN_DIR_NAME, LOGS_DIR_NAME};
use serde::Deserialize;

use crate::container::ServerContainerSpec;
use crate::launch::{LaunchContext, LAUNCH_LABEL, PROJECT_LABEL, ROLE_LABEL, USER_ASSISTANT_ROLE};
use crate::podman::Podman;
use crate::{Result, SandboxError};

pub const SERVER_ROLE: &str = "server";
pub const SERVER_LABEL: &str = "clyean.server";
pub const PERSISTENT_LABEL: &str = "clyean.persistent";
pub const REQUESTED_BY_LABEL: &str = "clyean.requested-by";
/// What the launch asked for, as JSON, which listings report and a repeated launch
/// compares.
pub const DEFINITION_LABEL: &str = "clyean.server-definition";

/// How long a stopped server may take to shut down before Podman kills it.
const STOP_GRACE_SECONDS: &str = "10";

const EXIT_MARKER: &str = "[clyean] the server's command exited with status ";

/// Runs a server's command (`$1`) with its output in its log (`$0`), which each launch
/// truncates, and ends the log with the command's exit status, because Podman removes the
/// container, and its status, when the command ends.  A stop signal reaches every process
/// of the command, so servers shut down cleanly.
const SERVER_WRAPPER: &str = r#"mkdir -p "${0%/*}" && exec >"$0" 2>&1 </dev/null
sh -c "$1" &
child=$!
trap 'trap "" INT TERM HUP; kill -TERM 0 2>/dev/null' INT TERM HUP
while :; do
	wait "$child"
	status=$?
	kill -0 "$child" 2>/dev/null || break
done
echo "[clyean] the server's command exited with status $status"
exit "$status""#;

/// One `--publish` of a server container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    /// `tcp` or `udp`.
    pub protocol: String,
    pub host_address: IpAddr,
    pub host_port: u16,
    pub guest_port: u16,
}

impl Publication {
    pub fn podman_argument(&self) -> String {
        let address = match self.host_address {
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => format!("[{address}]"),
        };
        format!(
            "{address}:{}:{}/{}",
            self.host_port, self.guest_port, self.protocol
        )
    }
}

/// A server container to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerContainer {
    pub name: String,
    pub command: String,
    /// The working directory's container path.
    pub workdir: String,
    pub persistent: bool,
    pub launch_id: String,
    /// The identifier of the agent that asked for the server.
    pub requested_by: String,
    pub publications: Vec<Publication>,
    /// The value of [`DEFINITION_LABEL`].
    pub definition: String,
}

/// The path of a server's log relative to the project directory.
pub fn server_log_relative_path(server: &str) -> String {
    format!("{CLYEAN_DIR_NAME}/{LOGS_DIR_NAME}/servers/{server}.log")
}

impl LaunchContext {
    pub fn server_container_name(&self, server: &str) -> String {
        format!("clyean-{}-server-{server}", self.project_id)
    }

    /// The container path of a server's working directory: `cwd` relative to the project
    /// directory, or an absolute path, inside the workspace either way; the project
    /// directory when `cwd` is empty.
    pub fn server_workdir(&self, cwd: &str) -> std::result::Result<String, String> {
        let paths = self.container_paths();
        let (mut resolved, relative) = match cwd.trim().strip_prefix('/') {
            Some(absolute) => (String::new(), absolute),
            None => (paths.project_dir().to_string(), cwd.trim()),
        };
        for part in relative.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(format!("the working directory {cwd:?} may not contain ..")),
                part => {
                    resolved.push('/');
                    resolved.push_str(part);
                }
            }
        }
        let workspace = paths.workspace_dir();
        if resolved == workspace || resolved.starts_with(&format!("{workspace}/")) {
            Ok(resolved)
        } else {
            Err(format!(
                "the working directory {cwd:?} is outside the workspace {workspace}"
            ))
        }
    }

    /// A server's log on this host.
    pub fn server_log_path(&self, server: &str) -> std::path::PathBuf {
        self.layout
            .logs_dir()
            .join("servers")
            .join(format!("{server}.log"))
    }

    /// The container of `server`, or `None` without a project network, which server
    /// containers need to be reachable.
    pub fn server_container_spec(&self, server: &ServerContainer) -> Option<ServerContainerSpec> {
        let network = self.network.as_ref()?;
        let paths = self.container_paths();
        let mut tmpfs_mounts = self.profile_masks();
        tmpfs_mounts.extend(self.host_state_masks(&paths));
        let mut environment = vec![("CLYEAN_SERVER".to_string(), server.name.clone())];
        environment.extend(self.common_environment(&paths));
        let mut labels = vec![self.sandbox.label()];
        labels.extend(
            [
                (PROJECT_LABEL, self.project_id.to_string()),
                (ROLE_LABEL, SERVER_ROLE.to_string()),
                (SERVER_LABEL, server.name.clone()),
                (LAUNCH_LABEL, server.launch_id.clone()),
                (PERSISTENT_LABEL, server.persistent.to_string()),
                (REQUESTED_BY_LABEL, server.requested_by.clone()),
                (DEFINITION_LABEL, server.definition.clone()),
            ]
            .map(|(key, value)| (key.to_string(), value)),
        );
        let log = format!(
            "{}/{}",
            paths.project_dir(),
            server_log_relative_path(&server.name)
        );
        Some(ServerContainerSpec {
            name: self.server_container_name(&server.name),
            rootfs: self.sandbox.root.clone(),
            workdir: server.workdir.clone(),
            labels,
            mounts: self.shared_mounts(&paths),
            tmpfs_mounts,
            environment,
            network: network.name().to_string(),
            alias: server.name.clone(),
            publish: server
                .publications
                .iter()
                .map(Publication::podman_argument)
                .collect(),
            extra_run_args: self.config.sandbox.podman_run_args.clone(),
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                SERVER_WRAPPER.into(),
                log,
                server.command.clone(),
            ],
        })
    }
}

/// A server container as `podman ps` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ListedServer {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Names", default)]
    pub names: Vec<String>,
    #[serde(rename = "State", default)]
    pub state: String,
    /// Start time in seconds since the Unix epoch.
    #[serde(rename = "StartedAt", default)]
    pub started_at: i64,
    #[serde(rename = "Labels", default)]
    labels: Option<HashMap<String, String>>,
}

impl ListedServer {
    pub fn container(&self) -> &str {
        self.names.first().map(String::as_str).unwrap_or(&self.id)
    }

    fn label(&self, key: &str) -> &str {
        self.labels
            .as_ref()
            .and_then(|labels| labels.get(key))
            .map(String::as_str)
            .unwrap_or_default()
    }

    pub fn server(&self) -> &str {
        self.label(SERVER_LABEL)
    }

    pub fn launch(&self) -> &str {
        self.label(LAUNCH_LABEL)
    }

    pub fn persistent(&self) -> bool {
        self.label(PERSISTENT_LABEL) == "true"
    }

    pub fn requested_by(&self) -> &str {
        self.label(REQUESTED_BY_LABEL)
    }

    pub fn definition(&self) -> &str {
        self.label(DEFINITION_LABEL)
    }

    pub fn is_running(&self) -> bool {
        self.state == "running"
    }
}

pub fn parse_server_list(text: &str) -> Result<Vec<ListedServer>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text)
        .map_err(|error| SandboxError::Invalid(format!("unexpected podman ps output: {error}")))
}

/// The server containers, running or not, that carry every `KEY=VALUE` label in `labels`
/// (none selects every project's).
pub fn list_servers(podman: &Podman, labels: &[String]) -> Result<Vec<ListedServer>> {
    let mut args = vec![
        "ps".to_string(),
        "--all".to_string(),
        "--filter".to_string(),
        format!("label={ROLE_LABEL}={SERVER_ROLE}"),
    ];
    for label in labels {
        args.push("--filter".to_string());
        args.push(format!("label={label}"));
    }
    args.extend(["--format".to_string(), "json".to_string()]);
    parse_server_list(&podman.output(args)?)
}

/// Stops and removes containers, giving each some seconds to shut down.
pub fn stop_containers(podman: &Podman, containers: &[String]) -> Result<()> {
    if containers.is_empty() {
        return Ok(());
    }
    let mut args = vec![
        "rm".to_string(),
        "--force".to_string(),
        "--ignore".to_string(),
        "--time".to_string(),
        STOP_GRACE_SECONDS.to_string(),
    ];
    args.extend(containers.iter().cloned());
    podman.output(args).map(drop)
}

/// The non-persistent servers whose `clyean` process is gone: no running User Assistant
/// container carries their launch.
pub fn select_stale<'a>(
    servers: &'a [ListedServer],
    live_launches: &[String],
) -> Vec<&'a ListedServer> {
    servers
        .iter()
        .filter(|server| !server.persistent())
        .filter(|server| !live_launches.iter().any(|launch| launch == server.launch()))
        .collect()
}

/// Stops the stale servers carrying every label in `labels` and returns their names.
pub fn stop_stale_servers(podman: &Podman, labels: &[String]) -> Result<Vec<String>> {
    let servers = list_servers(podman, labels)?;
    if servers.iter().all(ListedServer::persistent) {
        return Ok(Vec::new());
    }
    let launches = podman.output([
        "ps",
        "--filter",
        &format!("label={ROLE_LABEL}={USER_ASSISTANT_ROLE}"),
        "--format",
        &format!("{{{{index .Labels \"{LAUNCH_LABEL}\"}}}}"),
    ])?;
    let live: Vec<String> = launches
        .lines()
        .map(|line| line.trim().to_string())
        .collect();
    let stale = select_stale(&servers, &live);
    let containers: Vec<String> = stale
        .iter()
        .map(|server| server.container().to_string())
        .collect();
    stop_containers(podman, &containers)?;
    Ok(stale
        .iter()
        .map(|server| server.server().to_string())
        .collect())
}

/// The exit status a server's log ends with, once its command has ended.
pub fn exit_status_in_log(log: &str) -> Option<i32> {
    log.lines()
        .rev()
        .find_map(|line| line.strip_prefix(EXIT_MARKER))
        .and_then(|status| status.trim().parse().ok())
}

/// The last `count` lines of a log.
pub fn log_tail(log: &str, count: usize) -> String {
    let lines: Vec<&str> = log.lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{HostPathMapper, Topology};
    use crate::network::ProjectNetwork;
    use crate::roots::SandboxRoots;
    use crate::user::ContainerUser;
    use clyean_project::{
        ProjectConfig, ProjectDirectory, ProjectId, ProjectLayout, ProjectType, SandboxConfig,
        SandboxId,
    };
    use std::path::{Path, PathBuf};

    fn context(dir: &Path) -> LaunchContext {
        let project = dir.join("workspace").join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let directory =
            ProjectDirectory::resolve(Some(&project), Some(&dir.join("workspace"))).unwrap();
        let mut config = ProjectConfig::new(
            "0.1.0",
            ProjectType::SoftwareEngineeringProject,
            directory.workspace(),
            false,
            SandboxConfig::with_image("docker.io/library/ubuntu:latest"),
        );
        config.sandbox.podman_run_args = vec!["--memory".into(), "8g".into()];
        let project_id = ProjectId::of(directory.project());
        LaunchContext {
            podman: Podman::default(),
            layout: ProjectLayout::new(directory.project()),
            network: Some(ProjectNetwork::for_project(&project_id)),
            project_id,
            directory,
            config,
            user: ContainerUser::from_host_user_name("skyei"),
            clyean_version: "0.1.0".into(),
            herdr: None,
            sandbox: SandboxRoots::beside_graph_root("/home/skyei/.local/share/containers/storage")
                .location(
                    SandboxId::try_from("3f9c2a7d1e4b8c05".to_string()).unwrap(),
                    Topology::SharedKernel,
                ),
            host_paths: HostPathMapper::Identity,
            git_settings: Vec::new(),
            host_config_dir: PathBuf::from("/home/skyei/.config/clyean"),
        }
    }

    fn server() -> ServerContainer {
        ServerContainer {
            name: "web".into(),
            command: "npm run dev -- --host 0.0.0.0".into(),
            workdir: "/home/skyei/workspace/workspace/proj/app".into(),
            persistent: true,
            launch_id: "a68b8d72".into(),
            requested_by: "programmer".into(),
            publications: vec![
                Publication {
                    protocol: "tcp".into(),
                    host_address: "127.0.0.1".parse().unwrap(),
                    host_port: 3000,
                    guest_port: 5173,
                },
                Publication {
                    protocol: "tcp".into(),
                    host_address: "::1".parse().unwrap(),
                    host_port: 3000,
                    guest_port: 5173,
                },
            ],
            definition: "{}".into(),
        }
    }

    #[test]
    fn a_server_container_is_detached_named_labeled_published_and_logged() {
        let dir = tempfile::tempdir().unwrap();
        let context = context(dir.path());
        let spec = context.server_container_spec(&server()).unwrap();
        let args = spec.run_args();
        let project = context.project_id.to_string();
        assert_eq!(
            &args[..6],
            [
                "run",
                "--detach",
                "--rm",
                "--init",
                "--name",
                &format!("clyean-{project}-server-web")
            ]
        );
        for expected in [
            "clyean.sandbox=3f9c2a7d1e4b8c05".to_string(),
            format!("clyean.project={project}"),
            "clyean.role=server".into(),
            "clyean.server=web".into(),
            "clyean.launch=a68b8d72".into(),
            "clyean.persistent=true".into(),
            "clyean.requested-by=programmer".into(),
            "clyean.server-definition={}".into(),
            "127.0.0.1:3000:5173/tcp".into(),
            "[::1]:3000:5173/tcp".into(),
            "type=tmpfs,dst=/home/skyei/.omp/profiles,notmpcopyup".into(),
            "type=tmpfs,dst=/home/skyei/.omp/agent,notmpcopyup".into(),
            "CLYEAN_SERVER=web".into(),
            "HOME=/home/skyei".into(),
            "--memory".into(),
        ] {
            assert!(args.contains(&expected), "missing {expected}");
        }
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--network-alias" && pair[1] == "web"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--workdir" && pair[1].ends_with("/proj/app")));
        assert!(!args.iter().any(|arg| arg.starts_with("CLYEAN_AGENT=")
            || arg.starts_with("OMP_PROFILE=")
            || arg.starts_with("GIT_AUTHOR_NAME=")
            || arg.contains("_API_KEY")));
        assert!(!spec
            .mounts
            .iter()
            .any(|mount| mount.target.starts_with("/home/skyei/.omp")));
        let rootfs = args.iter().position(|arg| arg == "--rootfs").unwrap();
        let command = &args[rootfs + 2..];
        assert_eq!(&command[..2], ["/bin/sh", "-c"]);
        assert!(command[3].ends_with("/proj/.clyean/logs/servers/web.log"));
        assert_eq!(command[4], "npm run dev -- --host 0.0.0.0");
    }

    #[test]
    fn without_a_project_network_no_server_container_is_described() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = context(dir.path());
        context.network = None;
        assert!(context.server_container_spec(&server()).is_none());
    }

    #[test]
    fn working_directories_stay_inside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let context = context(dir.path());
        let project = "/home/skyei/workspace/workspace/proj";
        assert_eq!(context.server_workdir("").unwrap(), project);
        assert_eq!(
            context.server_workdir("app/./web/").unwrap(),
            format!("{project}/app/web")
        );
        assert_eq!(
            context
                .server_workdir("/home/skyei/workspace/workspace/other")
                .unwrap(),
            "/home/skyei/workspace/workspace/other"
        );
        assert!(context.server_workdir("../elsewhere").is_err());
        assert!(context.server_workdir("/etc").is_err());
        assert!(context
            .server_workdir("/home/skyei/workspace/workspace-other")
            .is_err());
    }

    #[test]
    fn listed_servers_expose_their_labels() {
        let text = r#"[{"Id":"abc","Names":["clyean-p-server-web"],"State":"running","StartedAt":1790446905,
            "Labels":{"clyean.server":"web","clyean.launch":"l1","clyean.persistent":"false","clyean.requested-by":"programmer","clyean.server-definition":"{\"command\":\"x\"}"}},
            {"Id":"def","Names":["clyean-p-server-db"],"State":"running","StartedAt":1790446906,"Labels":null}]"#;
        let servers = parse_server_list(text).unwrap();
        assert_eq!(servers[0].container(), "clyean-p-server-web");
        assert_eq!(servers[0].server(), "web");
        assert_eq!(servers[0].launch(), "l1");
        assert!(!servers[0].persistent());
        assert_eq!(servers[0].requested_by(), "programmer");
        assert_eq!(servers[0].definition(), r#"{"command":"x"}"#);
        assert!(servers[0].is_running());
        assert_eq!(servers[1].server(), "");
        assert!(parse_server_list("").unwrap().is_empty());
    }

    #[test]
    fn only_non_persistent_servers_of_gone_launches_are_stale() {
        let text = r#"[
            {"Id":"1","Names":["live"],"State":"running","Labels":{"clyean.launch":"l1","clyean.persistent":"false"}},
            {"Id":"2","Names":["gone"],"State":"running","Labels":{"clyean.launch":"l2","clyean.persistent":"false"}},
            {"Id":"3","Names":["kept"],"State":"running","Labels":{"clyean.launch":"l2","clyean.persistent":"true"}}]"#;
        let servers = parse_server_list(text).unwrap();
        let stale = select_stale(&servers, &["l1".to_string()]);
        let names: Vec<&str> = stale.iter().map(|server| server.container()).collect();
        assert_eq!(names, ["gone"]);
    }

    #[test]
    fn a_log_ends_with_the_exit_status_of_its_command() {
        let log = "listening on 5173\nerror: EADDRINUSE\n[clyean] the server's command exited with status 1\n";
        assert_eq!(exit_status_in_log(log), Some(1));
        assert_eq!(exit_status_in_log("still running\n"), None);
        assert_eq!(
            log_tail(log, 2),
            "error: EADDRINUSE\n[clyean] the server's command exited with status 1"
        );
        assert_eq!(log_tail("one", 5), "one");
    }

    #[test]
    fn ipv6_publications_are_bracketed() {
        let publication = Publication {
            protocol: "udp".into(),
            host_address: "::".parse().unwrap(),
            host_port: 5353,
            guest_port: 5300,
        };
        assert_eq!(publication.podman_argument(), "[::]:5353:5300/udp");
    }
}
