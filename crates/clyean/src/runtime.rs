// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Host-side wiring shared by the commands: project resolution, the sandbox launch
//! context, and the production implementations of the orchestrator's dependencies
//! (diagram rendering and sub-agent sessions inside Podman containers).

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clyean_agents::AgentId;
use clyean_harness::session::BoxFuture;
use clyean_harness::{AgentSessionDriver, HarnessClient, HarnessSession};
use clyean_orchestrator::agents::AgentSessionFactory;
use clyean_orchestrator::scaffold::{ensure_sandbox, PendingScaffold, SandboxInputs};
use clyean_orchestrator::service::DiagramRenderer;
use clyean_plantuml::render::render_directory;
use clyean_plantuml::RenderReport;
use clyean_project::{ProjectConfig, ProjectDirectory, ProjectId, ProjectLayout, SandboxConfig};
use clyean_sandbox::launch::cache_dir;
use clyean_sandbox::rootfs::RootfsMarker;
use clyean_sandbox::{ContainerUser, HerdrHostContext, LaunchContext, LaunchRole, Podman};

use crate::cli::{ProjectArgs, YesNo, VERSION};

const HARNESS_READY_TIMEOUT: Duration = Duration::from_secs(180);
const SUB_AGENT_TURN_TIMEOUT: Duration = Duration::from_secs(4 * 60 * 60);

pub fn init_tracing(verbose: bool) {
    use tracing_subscriber::EnvFilter;
    let default_level = if verbose { "info" } else { "warn" };
    let filter =
        EnvFilter::try_from_env("CLYEAN_LOG").unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

/// The resolved project and the host facilities every command needs.
pub struct ProjectRuntime {
    pub directory: ProjectDirectory,
    pub layout: ProjectLayout,
    pub project_id: ProjectId,
    pub podman: Podman,
    pub user: ContainerUser,
    pub herdr: Option<HerdrHostContext>,
    pub cache_dir: PathBuf,
}

impl ProjectRuntime {
    pub fn resolve(args: &ProjectArgs) -> Result<Self> {
        let directory = ProjectDirectory::resolve(args.cwd.as_deref(), args.workspace.as_deref())?;
        let layout = ProjectLayout::new(directory.project());
        let project_id = ProjectId::of(directory.project());
        Ok(Self {
            directory,
            layout,
            project_id,
            podman: Podman::default(),
            user: ContainerUser::from_environment(),
            herdr: HerdrHostContext::detect_from_process_environment(),
            cache_dir: cache_dir(),
        })
    }

    pub fn is_scaffolded(&self) -> bool {
        self.layout.is_scaffolded()
    }

    pub fn load_config(&self) -> Result<ProjectConfig> {
        ProjectConfig::load(&self.layout).context("loading .clyean/project.json")
    }

    /// The sandbox settings of a project that is not scaffolded yet, from flags and, for
    /// the worktree question, an interactive prompt when a terminal is attached.
    pub fn pending_scaffold(
        &self,
        worktrees: Option<YesNo>,
        image: Option<String>,
        mounts: Vec<PathBuf>,
    ) -> Result<PendingScaffold> {
        let mut pending = PendingScaffold::default();
        if let Some(image) = image {
            pending.sandbox.image = image;
        }
        pending.sandbox.mounts = mounts
            .into_iter()
            .map(|path| {
                std::fs::canonicalize(&path)
                    .with_context(|| format!("resolving mount {}", path.display()))
            })
            .collect::<Result<Vec<_>>>()?;
        pending.use_worktrees = match worktrees {
            Some(YesNo::Yes) => true,
            Some(YesNo::No) => false,
            None => ask_worktree_question()?,
        };
        Ok(pending)
    }

    /// The sandbox configuration in effect: the project's when scaffolded, else pending.
    pub fn sandbox_config(&self, pending: &PendingScaffold) -> Result<SandboxConfig> {
        if self.is_scaffolded() {
            Ok(self.load_config()?.sandbox)
        } else {
            Ok(pending.sandbox.clone())
        }
    }

    pub async fn ensure_sandbox(&self, sandbox: &SandboxConfig) -> Result<(RootfsMarker, bool)> {
        let inputs = SandboxInputs {
            podman: &self.podman,
            layout: &self.layout,
            user: &self.user,
            sandbox,
            clyean_version: VERSION,
            cache_dir: &self.cache_dir,
        };
        ensure_sandbox(&inputs)
            .await
            .context("preparing the sandbox root filesystem")
    }

    pub fn launch_context(
        &self,
        config: ProjectConfig,
        orchestrator_socket: Option<PathBuf>,
    ) -> LaunchContext {
        LaunchContext {
            podman: self.podman.clone(),
            directory: self.directory.clone(),
            layout: self.layout.clone(),
            project_id: self.project_id.clone(),
            config,
            user: self.user.clone(),
            clyean_version: VERSION.to_string(),
            herdr: self.herdr.clone(),
            orchestrator_socket,
        }
    }

    /// A configuration usable before the project is scaffolded, for maintenance containers.
    pub fn provisional_config(&self, pending: &PendingScaffold) -> ProjectConfig {
        ProjectConfig::new(
            VERSION,
            clyean_project::ProjectType::SoftwareEngineeringProject,
            self.directory.workspace(),
            pending.use_worktrees,
            pending.sandbox.clone(),
        )
    }
}

fn ask_worktree_question() -> Result<bool> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        eprintln!("No terminal is attached; Git worktrees stay disabled for this project (pass --worktrees yes to enable them).");
        return Ok(false);
    }
    println!("This project has no Clyean scaffold yet.");
    println!("May Clyean use Git worktrees for this project?  Worktrees are preferable: they let agents work concurrently, which improves performance. [Y/n] ");
    print!("> ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(!matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "n" | "no"
    ))
}

/// Renders `.clyean/architecture` inside a maintenance container of the project.
pub struct SandboxRenderer {
    context: LaunchContext,
}

impl SandboxRenderer {
    pub fn new(context: LaunchContext) -> Self {
        Self { context }
    }
}

impl DiagramRenderer for SandboxRenderer {
    fn render<'a>(&'a self) -> BoxFuture<'a, clyean_orchestrator::Result<RenderReport>> {
        Box::pin(async move {
            let runner = self.context.maintenance_runner();
            let host_dir = self.context.layout.architecture_dir();
            let container_dir = self.context.container_paths().architecture_dir();
            tokio::task::spawn_blocking(move || {
                render_directory(&runner, &host_dir, &container_dir)
            })
            .await
            .map_err(|e| {
                clyean_orchestrator::OrchestratorError::Workflow(format!("render task failed: {e}"))
            })?
            .map_err(|e| {
                clyean_orchestrator::OrchestratorError::io("rendering architecture diagrams", e)
            })
        })
    }
}

/// Opens sub-agent sessions as harness processes in their own sandbox containers.
pub struct SandboxSessionFactory {
    context: LaunchContext,
}

impl SandboxSessionFactory {
    pub fn new(context: LaunchContext) -> Self {
        Self { context }
    }
}

impl AgentSessionFactory for SandboxSessionFactory {
    fn open<'a>(
        &'a self,
        agent: AgentId,
        work_id: &'a str,
        resume_session_file: Option<&'a str>,
    ) -> BoxFuture<'a, clyean_orchestrator::Result<Box<dyn AgentSessionDriver>>> {
        Box::pin(async move {
            let args = self
                .context
                .sub_agent_harness_args(agent, resume_session_file);
            let role = LaunchRole::SubAgent {
                work_id: work_id.to_string(),
            };
            let spec = self.context.agent_container_spec(agent, role, args);
            let _ = self.context.podman.remove_container(&spec.name);
            let mut command = tokio::process::Command::new(self.context.podman.binary());
            command.args(spec.run_args());
            tracing::info!(target: "clyean::launch", agent = agent.id(), container = %spec.name, "starting sub-agent");
            let client = HarnessClient::spawn(command, HARNESS_READY_TIMEOUT).await?;
            Ok(
                Box::new(HarnessSession::new(client, SUB_AGENT_TURN_TIMEOUT))
                    as Box<dyn AgentSessionDriver>,
            )
        })
    }
}
