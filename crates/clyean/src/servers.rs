// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The host's side of server containers: the orchestrator's server manager decides, and
//! this module starts, probes, lists, and stops the containers through Podman.

use std::net::IpAddr;

use clyean_harness::session::BoxFuture;
use clyean_orchestrator::servers::{ContainerProbe, RunningServer, ServerRuntime};
use clyean_orchestrator::ServerManager;
use clyean_sandbox::launch::PROJECT_LABEL;
use clyean_sandbox::server::{list_servers, stop_containers, stop_stale_servers, ServerContainer};
use clyean_sandbox::{LaunchContext, Podman};

/// Prints every `/proc/net` table a readiness check reads, each after a `== <table>` line.
const SOCKET_TABLES_SCRIPT: &str = r#"for table in tcp tcp6 udp udp6 sctp/eps; do echo "== $table"; cat "/proc/net/$table" 2>/dev/null; done"#;

/// The project's server containers, through Podman.
pub struct PodmanServerRuntime {
    context: LaunchContext,
}

impl PodmanServerRuntime {
    pub fn new(context: LaunchContext) -> Self {
        Self { context }
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

impl ServerRuntime for PodmanServerRuntime {
    fn unavailable(&self) -> Option<String> {
        self.context.network.is_none().then(|| {
            "server containers need the project network, which could not be created; see the warning clyean printed when it started".to_string()
        })
    }

    fn workdir(&self, cwd: &str) -> Result<String, String> {
        self.context.server_workdir(cwd)
    }

    fn start<'a>(&'a self, server: &'a ServerContainer) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let spec = self
                .context
                .server_container_spec(server)
                .ok_or_else(|| self.unavailable().unwrap_or_default())?;
            let podman = self.context.podman.clone();
            blocking(move || {
                // Without --force, only a container of the name that no longer runs goes:
                // one a launch left behind would otherwise block the name.
                let _ = podman.output(["rm", "--ignore", &spec.name]);
                podman
                    .output(spec.run_args())
                    .map_err(|error| error.to_string())?;
                Ok(spec.name)
            })
            .await
        })
    }

    fn list(&self) -> BoxFuture<'_, Result<Vec<RunningServer>, String>> {
        Box::pin(async move {
            let podman = self.context.podman.clone();
            let project = format!("{PROJECT_LABEL}={}", self.context.project_id);
            blocking(move || {
                let listed =
                    list_servers(&podman, &[project]).map_err(|error| error.to_string())?;
                Ok(listed
                    .iter()
                    .filter(|server| server.is_running())
                    .map(|server| RunningServer {
                        name: server.server().to_string(),
                        container: server.container().to_string(),
                        persistent: server.persistent(),
                        launch_id: server.launch().to_string(),
                        requested_by: server.requested_by().to_string(),
                        started_at: server.started_at,
                        definition: serde_json::from_str(server.definition()).ok(),
                    })
                    .collect())
            })
            .await
        })
    }

    fn probe<'a>(
        &'a self,
        container: &'a str,
    ) -> BoxFuture<'a, Result<Option<ContainerProbe>, String>> {
        Box::pin(async move {
            let podman = self.context.podman.clone();
            let container = container.to_string();
            blocking(move || probe(&podman, &container)).await
        })
    }

    fn stop<'a>(&'a self, containers: &'a [String]) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let podman = self.context.podman.clone();
            let containers = containers.to_vec();
            blocking(move || {
                stop_containers(&podman, &containers).map_err(|error| error.to_string())
            })
            .await
        })
    }

    fn log(&self, server: &str) -> String {
        std::fs::read_to_string(self.context.server_log_path(server)).unwrap_or_default()
    }
}

/// A running container's addresses and socket tables; `None` once it has ended.
fn probe(podman: &Podman, container: &str) -> Result<Option<ContainerProbe>, String> {
    let ended = |error: clyean_sandbox::SandboxError| match podman.container_is_running(container) {
        Ok(false) => Ok(None),
        _ => Err(error.to_string()),
    };
    let inspected = match podman.output([
        "container",
        "inspect",
        "--format",
        "{{.State.Status}} {{range .NetworkSettings.Networks}}{{.IPAddress}} {{.GlobalIPv6Address}} {{end}}",
        container,
    ]) {
        Ok(inspected) => inspected,
        Err(error) => return ended(error),
    };
    let mut words = inspected.split_whitespace();
    if words.next() != Some("running") {
        return Ok(None);
    }
    let mut addresses: Vec<IpAddr> = words.filter_map(|word| word.parse().ok()).collect();
    addresses.sort_by_key(IpAddr::is_ipv6);
    match podman.output(["exec", container, "/bin/sh", "-c", SOCKET_TABLES_SCRIPT]) {
        Ok(socket_tables) => Ok(Some(ContainerProbe {
            addresses,
            socket_tables,
        })),
        Err(error) => ended(error),
    }
}

/// Stops the project's servers that `clyean` processes which have ended left running.
pub fn stop_servers_left_behind(context: &LaunchContext) {
    let project = format!("{PROJECT_LABEL}={}", context.project_id);
    match stop_stale_servers(&context.podman, &[project]) {
        Ok(stopped) if !stopped.is_empty() => eprintln!(
            "Stopped servers that ended clyean processes left running: {}",
            stopped.join(", ")
        ),
        Ok(_) => {}
        Err(error) => {
            tracing::debug!(target: "clyean::servers", %error, "could not look for servers left behind")
        }
    }
}

/// Stops this process's non-persistent servers as it exits, and names the persistent
/// servers that keep running.
pub async fn finish(servers: &ServerManager) {
    match servers.finish().await {
        Ok(remaining) => {
            let persistent: Vec<String> = remaining
                .into_iter()
                .filter(|server| server.persistent)
                .map(|server| server.name)
                .collect();
            if !persistent.is_empty() {
                eprintln!(
                    "Persistent servers keep running: {}.  Stop them with `clyean server stop <name>` or `clyean server stop --all`.",
                    persistent.join(", ")
                );
            }
        }
        Err(error) => eprintln!(
            "warning: this clyean process's servers could not be stopped ({error}); the next clyean launch or `clyean sandbox prune` stops them"
        ),
    }
}
