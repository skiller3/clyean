// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! The default command: prepare the host scaffold and sandbox, then run this invocation's
//! own User Assistant in a container that ends with the invocation.  The container reaches
//! the orchestrator, which runs in this process, only through the bridge.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clyean_agents::AgentId;
use clyean_bridge::host::{self as bridge_host, ConnectFuture, Connector, LocalStream};
use clyean_orchestrator::scaffold::{prepare_host_files, project_agent_profiles, PendingScaffold};
use clyean_orchestrator::server::handle_connection;
use clyean_orchestrator::service::{
    OrchestratorService, ProjectServices, ProjectState, UnscaffoldedProject,
};
use clyean_orchestrator::CredentialAuthority;
use clyean_project::{LaunchId, PlanCatalog, ProjectId};
use clyean_sandbox::launch::{PROJECT_LABEL, ROLE_LABEL, USER_ASSISTANT_ROLE};
use clyean_sandbox::{AgentContainerSpec, LaunchContext, LaunchRole, Podman};
use tokio::io::AsyncBufReadExt;
use tokio::process::{Child, Command};

use crate::cli::{LaunchArgs, ProjectArgs, VERSION};
use crate::runtime::{ProjectRuntime, SandboxRenderer, SandboxSessionFactory};

/// How long the User Assistant's container may take to start.
const CONTAINER_START_TIMEOUT: Duration = Duration::from_secs(120);
/// How long the bridge may take to introduce itself once its session is opened.
const BRIDGE_START_TIMEOUT: Duration = Duration::from_secs(60);

pub async fn run(project: &ProjectArgs, args: LaunchArgs) -> Result<i32> {
    let runtime = ProjectRuntime::resolve(project)?;
    let pending = if runtime.is_scaffolded() {
        PendingScaffold::default()
    } else {
        runtime.pending_scaffold(args.worktrees, args.image.clone(), args.mounts.clone())?
    };
    let (git, report) = prepare_host_files(&runtime.directory, &runtime.layout)?;
    if report.git_initialized {
        eprintln!(
            "Initialized a Git repository in {}",
            runtime.directory.project().display()
        );
    }
    let sandbox = runtime.sandbox_config(&pending)?;
    let (marker, provisioned) = runtime.ensure_sandbox(&sandbox).await?;
    if provisioned {
        eprintln!(
            "Provisioned the Podman sandbox (harness {}).",
            marker.harness_version
        );
    }
    project_agent_profiles(&runtime.layout, &runtime.user)?;
    let bridge_binary = runtime.resolve_bridge_binary().await?;

    let config = if runtime.is_scaffolded() {
        runtime.load_config()?
    } else {
        runtime.provisional_config(&pending)
    };
    let context = runtime.launch_context(config.clone());
    let renderer = Arc::new(SandboxRenderer::new(context.clone()));
    let credentials = Arc::new(CredentialAuthority::default());
    let factory = Arc::new(SandboxSessionFactory::new(
        context.clone(),
        credentials.clone(),
    ));
    let state = if runtime.is_scaffolded() {
        ProjectState::Scaffolded(Arc::new(ProjectServices {
            directory: runtime.directory.clone(),
            layout: runtime.layout.clone(),
            config,
            git,
            plans: PlanCatalog::new(&runtime.layout),
            renderer,
            factory,
            clyean_version: VERSION.to_string(),
        }))
    } else {
        ProjectState::Unscaffolded(Box::new(UnscaffoldedProject {
            directory: runtime.directory.clone(),
            layout: runtime.layout.clone(),
            git,
            pending,
            renderer,
            factory,
            clyean_version: VERSION.to_string(),
        }))
    };
    let service =
        Arc::new(OrchestratorService::new(state, VERSION).with_credential_authority(credentials));

    if args.r#continue {
        warn_if_another_launch_is_running(&runtime.podman, &runtime.project_id);
    }
    let role = LaunchRole::UserAssistant {
        launch_id: LaunchId::generate(),
        bridge_binary,
    };
    let harness_args = user_assistant_harness_args(&context, &args);
    let mut spec = context.agent_container_spec(AgentId::UserAssistant, role, harness_args);
    spec.tty = !args.print;
    let connector = Arc::new(LaunchConnector {
        service,
        herdr_socket: runtime
            .herdr
            .as_ref()
            .map(|herdr| herdr.socket_path.clone()),
    });
    run_user_assistant(&context, &spec, connector).await
}

fn user_assistant_harness_args(context: &LaunchContext, args: &LaunchArgs) -> Vec<String> {
    let mut harness_args = context.base_harness_args(AgentId::UserAssistant);
    if args.r#continue {
        harness_args.push("--continue".into());
    }
    if let Some(session) = &args.resume {
        harness_args.push("--resume".into());
        if !session.is_empty() {
            harness_args.push(session.clone());
        }
    }
    if let Some(model) = &args.model {
        harness_args.push("--model".into());
        harness_args.push(model.clone());
    }
    if args.no_session {
        harness_args.push("--no-session".into());
    }
    if args.print {
        harness_args.push("--print".into());
    }
    if !args.prompt.is_empty() {
        harness_args.push("--".into());
        harness_args.push(args.prompt.join(" "));
    }
    harness_args
}

/// `--continue` opens the project's most recent session, which another running invocation
/// may be using; the harness does not support two processes in one session.
fn warn_if_another_launch_is_running(podman: &Podman, project_id: &ProjectId) {
    let project = format!("label={PROJECT_LABEL}={project_id}");
    let role = format!("label={ROLE_LABEL}={USER_ASSISTANT_ROLE}");
    let running = podman.output([
        "ps",
        "--filter",
        &project,
        "--filter",
        &role,
        "--format",
        "{{.Names}}",
    ]);
    if matches!(running, Ok(names) if !names.trim().is_empty()) {
        eprintln!(
            "warning: another clyean process is running for this project; --continue opens the project's most recent session, which that process may be using."
        );
    }
}

/// Runs the User Assistant's container in the foreground, opens its bridge once it runs,
/// and returns the harness's exit status when the container ends.
async fn run_user_assistant(
    context: &LaunchContext,
    spec: &AgentContainerSpec,
    connector: Arc<LaunchConnector>,
) -> Result<i32> {
    ignore_interrupts();
    let podman = &context.podman;
    let _cleanup = ContainerCleanup {
        podman: podman.clone(),
        name: spec.name.clone(),
    };
    let mut container = Command::new(podman.binary())
        .args(spec.run_args())
        .spawn()
        .context("starting the User Assistant's container")?;
    tracing::debug!(target: "clyean::launch", container = %spec.name, "container starting");
    if !wait_until_running(podman, &spec.name, &mut container).await? {
        let status = container.wait().await?;
        return Ok(status.code().unwrap_or(1));
    }
    tracing::debug!(target: "clyean::launch", container = %spec.name, "container running");
    let bridge = match open_bridge(context, &spec.name, connector).await {
        Ok(bridge) => bridge,
        Err(error) => {
            let _ = podman.remove_container(&spec.name);
            let _ = container.wait().await;
            return Err(error.context("opening the User Assistant's bridge"));
        }
    };
    let status = container
        .wait()
        .await
        .context("waiting for the User Assistant")?;
    bridge.close().await;
    Ok(status.code().unwrap_or(1))
}

/// Waits until Podman reports the container running, returning `false` when `podman run`
/// ends first.
async fn wait_until_running(podman: &Podman, name: &str, container: &mut Child) -> Result<bool> {
    let deadline = Instant::now() + CONTAINER_START_TIMEOUT;
    loop {
        if container.try_wait()?.is_some() {
            return Ok(false);
        }
        let state = Command::new(podman.binary())
            .args([
                "container",
                "inspect",
                "--format",
                "{{.State.Status}}",
                name,
            ])
            .stderr(std::process::Stdio::null())
            .output()
            .await?;
        if state.status.success() && String::from_utf8_lossy(&state.stdout).trim() == "running" {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "the User Assistant's container {name} did not start within {} seconds",
                CONTAINER_START_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The running `podman exec` session of the bridge and the task that serves it.
struct OpenBridge {
    session: Child,
    served: tokio::task::JoinHandle<io::Result<()>>,
}

impl OpenBridge {
    async fn close(mut self) {
        let _ = self.session.start_kill();
        let _ = self.session.wait().await;
        self.served.abort();
    }
}

async fn open_bridge(
    context: &LaunchContext,
    container_name: &str,
    connector: Arc<LaunchConnector>,
) -> Result<OpenBridge> {
    let mut command = Command::new(context.podman.binary());
    command
        .args(context.bridge_exec_args(container_name))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Keeps terminal signals, such as Ctrl-C in print mode, away from the session: only
    // this process ending may end the bridge.
    #[cfg(unix)]
    command.process_group(0);
    let mut session = command
        .spawn()
        .context("starting the bridge's podman exec session")?;
    let (Some(input), Some(output), Some(errors)) = (
        session.stdin.take(),
        session.stdout.take(),
        session.stderr.take(),
    ) else {
        return Err(anyhow!("the bridge session has no standard streams"));
    };
    // Podman reports the session's end when the container goes first, which is the normal
    // end of every launch, so the session's messages are informational.
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(errors).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::info!(target: "clyean::bridge", "{line}");
        }
    });
    let (introduced, version) = tokio::sync::oneshot::channel();
    let served = tokio::spawn(bridge_host::serve(output, input, connector, introduced));
    match tokio::time::timeout(BRIDGE_START_TIMEOUT, version).await {
        Ok(Ok(version)) => {
            tracing::info!(target: "clyean::bridge", %version, container = %container_name, "bridge open");
            Ok(OpenBridge { session, served })
        }
        Ok(Err(_)) => {
            let error = match served.await {
                Ok(Err(error)) => anyhow!(error),
                _ => anyhow!("the bridge ended before introducing itself"),
            };
            Err(error)
        }
        Err(_) => Err(anyhow!(
            "the bridge did not start within {} seconds",
            BRIDGE_START_TIMEOUT.as_secs()
        )),
    }
}

/// Connects the bridge's channels on the host: the orchestrator runs in this process, and
/// the Herdr socket is the one Herdr gave this invocation's pane.
struct LaunchConnector {
    service: Arc<OrchestratorService>,
    herdr_socket: Option<PathBuf>,
}

impl Connector for LaunchConnector {
    fn connect<'a>(&'a self, channel: &'a str) -> ConnectFuture<'a> {
        Box::pin(async move {
            match channel {
                clyean_bridge::ORCHESTRATOR_CHANNEL => Ok(self.orchestrator_stream()),
                clyean_bridge::HERDR_CHANNEL => self.herdr_stream().await,
                other => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no channel named {other}"),
                )),
            }
        })
    }
}

impl LaunchConnector {
    fn orchestrator_stream(&self) -> Box<dyn LocalStream> {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let service = self.service.clone();
        tokio::spawn(async move {
            let (reader, writer) = tokio::io::split(server);
            if let Err(error) = handle_connection(service, reader, writer).await {
                tracing::debug!(target: "clyean::orchestrator", %error, "connection ended with an error");
            }
        });
        Box::new(client)
    }

    #[cfg(unix)]
    async fn herdr_stream(&self) -> io::Result<Box<dyn LocalStream>> {
        let path = self
            .herdr_socket
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not inside a Herdr pane"))?;
        let stream = tokio::net::UnixStream::connect(path).await?;
        Ok(Box::new(stream))
    }

    #[cfg(not(unix))]
    async fn herdr_stream(&self) -> io::Result<Box<dyn LocalStream>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the Herdr channel is not available on this platform yet",
        ))
    }
}

/// Removes the User Assistant's container when this invocation ends, for a Podman that
/// could not remove it itself.
struct ContainerCleanup {
    podman: Podman,
    name: String,
}

impl Drop for ContainerCleanup {
    fn drop(&mut self) {
        let _ = self
            .podman
            .command(["rm", "--force", "--ignore", &self.name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Ctrl-C belongs to the User Assistant, which Podman forwards it to, not to this process.
fn ignore_interrupts() {
    tokio::spawn(async {
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                break;
            }
        }
    });
}
