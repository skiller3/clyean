// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

//! Host-side wiring shared by the commands: project resolution, the sandbox launch
//! context, and the production implementations of the orchestrator's dependencies
//! (diagram rendering and sub-agent sessions inside Podman containers).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clyean_agents::profile::container_credentials_bundle_path;
use clyean_agents::{AgentId, AgentNeeds};
use clyean_harness::session::BoxFuture;
use clyean_harness::{AgentSessionDriver, HarnessClient, HarnessSession, UiRequestHandler};
use clyean_orchestrator::agents::AgentSessionFactory;
use clyean_orchestrator::credentials::{plan_delivery, CredentialRenewals, Delivery};
use clyean_orchestrator::scaffold::{ensure_sandbox, PendingScaffold, SandboxInputs};
use clyean_orchestrator::service::DiagramRenderer;
use clyean_orchestrator::CredentialAuthority;
use clyean_plantuml::render::render_directory;
use clyean_plantuml::RenderReport;
use clyean_project::{
    ProjectConfig, ProjectDirectory, ProjectId, ProjectLayout, SandboxConfig, SandboxId,
};
use clyean_sandbox::launch::{cache_dir, BUILTIN_PASSTHROUGH_PATTERNS};
use clyean_sandbox::provisioning::{resolve_sandbox_executable, BRIDGE};
use clyean_sandbox::rootfs::RootfsMarker;
use clyean_sandbox::{
    ContainerUser, HerdrHostContext, HostOs, HostPathMapper, LaunchContext, LaunchRole, Podman,
    PodmanEnvironment, SandboxArchive, SandboxLocation, Topology,
};

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

    /// Asks Podman about its environment, stops when Podman is older than this platform
    /// supports, prints its warnings, and locates the project's sandbox root filesystem,
    /// recording a sandbox identifier when the project has none.
    pub fn podman_sandbox(&self) -> Result<PodmanSandbox> {
        let host = HostOs::current();
        let environment = PodmanEnvironment::detect(&self.podman, host)
            .context("asking Podman about its environment")?;
        for warning in environment.check(host)? {
            eprintln!("warning: {warning}");
        }
        let id = SandboxId::load_or_create(&self.layout)?;
        let location = environment
            .sandbox_roots()
            .location(id, environment.topology);
        Ok(PodmanSandbox {
            host_paths: HostPathMapper::new(environment.topology, host),
            environment,
            location,
        })
    }

    pub async fn ensure_sandbox(
        &self,
        podman_sandbox: &PodmanSandbox,
        sandbox: &SandboxConfig,
    ) -> Result<(RootfsMarker, bool)> {
        let inputs = SandboxInputs {
            podman: &self.podman,
            environment: &podman_sandbox.environment,
            location: &podman_sandbox.location,
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
        podman_sandbox: &PodmanSandbox,
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
            sandbox: podman_sandbox.location.clone(),
            host_paths: podman_sandbox.host_paths,
            git_settings: ["core.autocrlf", "core.eol"]
                .into_iter()
                .filter_map(|key| {
                    clyean_git::host_setting(key).map(|value| (key.to_string(), value))
                })
                .collect(),
        }
    }

    /// The Herdr context for the User Assistant's container.  On a Podman machine, the
    /// host's own `herdr` cannot run in a Linux container, so its Linux build replaces it,
    /// or no executable is mounted when that build cannot be fetched.
    pub async fn container_herdr(
        &self,
        podman_sandbox: &PodmanSandbox,
    ) -> Option<HerdrHostContext> {
        let mut herdr = self.herdr.clone()?;
        if podman_sandbox.environment.topology != Topology::VirtualMachine {
            return Some(herdr);
        }
        let host_bin = herdr.bin_path.take()?;
        let Some(version) = clyean_sandbox::herdr::host_version(&host_bin) else {
            return Some(herdr);
        };
        let arch = podman_sandbox.environment.arch_tag().ok()?;
        match clyean_sandbox::herdr::linux_cli(&version, arch, &self.cache_dir).await {
            Ok(path) => herdr.bin_path = Some(path),
            Err(error) => eprintln!(
                "warning: the Herdr CLI is not available inside the User Assistant's container ({error})"
            ),
        }
        Some(herdr)
    }

    /// The host copy of the bridge for the architecture of the kernel that runs the
    /// containers, which the User Assistant's container mounts read-only.
    pub async fn resolve_bridge_binary(&self, podman_sandbox: &PodmanSandbox) -> Result<String> {
        let bridge = resolve_sandbox_executable(
            &BRIDGE,
            None,
            VERSION,
            podman_sandbox.environment.arch_tag()?,
            &self.cache_dir,
        )
        .await
        .context("finding the bridge executable")?;
        tracing::info!(target: "clyean::launch", bridge = %bridge.path.display(), origin = ?bridge.origin, "bridge executable");
        Ok(podman_sandbox.host_paths.map(&bridge.path))
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

/// What Podman reported about itself and where the project's sandbox lives on the Podman
/// host.
#[derive(Debug, Clone)]
pub struct PodmanSandbox {
    pub environment: PodmanEnvironment,
    pub location: SandboxLocation,
    pub host_paths: HostPathMapper,
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

/// Opens sub-agent sessions as harness processes in their own sandbox containers, each
/// with the credential copies and host variables it needs.
pub struct SandboxSessionFactory {
    context: LaunchContext,
    credentials: Arc<CredentialAuthority>,
}

impl SandboxSessionFactory {
    pub fn new(context: LaunchContext, credentials: Arc<CredentialAuthority>) -> Self {
        Self {
            context,
            credentials,
        }
    }

    async fn delivery(&self, agent: AgentId) -> clyean_orchestrator::Result<Delivery> {
        if !self.credentials.is_connected() {
            tracing::info!(target: "clyean::credentials", agent = agent.id(), "no User Assistant is running, so the agent receives the host's provider variables and no credential copies");
            return Ok(Delivery::without_authority(BUILTIN_PASSTHROUGH_PATTERNS));
        }
        let needs = AgentNeeds::for_agent(&self.context.layout, agent)?;
        let host_variables: HashSet<String> = std::env::vars().map(|(name, _)| name).collect();
        plan_delivery(&self.credentials, agent, &needs, &host_variables).await
    }

    /// Writes the bundle into the agent's own profile, readable by its owner only.
    fn write_bundle(
        &self,
        agent: AgentId,
        bundle: &serde_json::Value,
    ) -> clyean_orchestrator::Result<()> {
        let mut archive = SandboxArchive::new();
        archive.file(
            &container_credentials_bundle_path(self.context.user.name(), agent),
            bundle.to_string(),
            0o600,
        );
        self.context
            .sandbox
            .fs(&self.context.podman)
            .write(archive)?;
        Ok(())
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
            let delivery = self.delivery(agent).await?;
            self.write_bundle(agent, &delivery.bundle)?;
            let mut args = self
                .context
                .sub_agent_harness_args(agent, resume_session_file);
            if let Some(model) = &delivery.model {
                args.push("--model".to_string());
                args.push(model.clone());
            }
            let role = LaunchRole::SubAgent {
                work_id: work_id.to_string(),
                secrets: delivery.secrets.clone(),
            };
            let spec = self.context.agent_container_spec(agent, role, args);
            let _ = self.context.podman.remove_container(&spec.name);
            let mut command = tokio::process::Command::new(self.context.podman.binary());
            command.args(spec.run_args());
            let renewals: Option<Arc<dyn UiRequestHandler>> =
                self.credentials.is_connected().then(|| {
                    Arc::new(CredentialRenewals {
                        authority: self.credentials.clone(),
                        agent,
                        providers: delivery.providers.clone(),
                        mcp_servers: delivery.mcp_servers.clone(),
                    }) as Arc<dyn UiRequestHandler>
                });
            tracing::info!(target: "clyean::launch", agent = agent.id(), container = %spec.name, model = ?delivery.model, "starting sub-agent");
            let client =
                HarnessClient::spawn_with_ui_handler(command, HARNESS_READY_TIMEOUT, renewals)
                    .await?;
            Ok(
                Box::new(HarnessSession::new(client, SUB_AGENT_TURN_TIMEOUT))
                    as Box<dyn AgentSessionDriver>,
            )
        })
    }
}
